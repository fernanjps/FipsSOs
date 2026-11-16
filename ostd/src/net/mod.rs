//! FipSOs Ring 0 Network Subsystem (`ostd::net`): PCIe Intel `e1000`/`e1000e` DMA Driver
//! + Ethernet II / ARP / IPv4 / ICMP Echo / TCP Socket Transport Stack.

pub mod arp_ipv4;
pub mod dhcp;
pub mod e1000;
pub mod remote_shell;
pub mod tcp_client;
pub mod tcp_server;

pub use arp_ipv4::*;
pub use remote_shell::*;
pub use tcp_client::*;

/// Backward-compatible `ostd::net::tcp` namespace re-exporting client and remote console APIs.
pub mod tcp {
    pub use super::remote_shell::*;
    pub use super::tcp_client::*;
    pub use super::tcp_server::*;
}

use alloc::format;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU16, Ordering};

use crate::sync::SpinLock;
use e1000::{E1000Controller, NicHardwareSnapshot, PKT_BUF_SIZE};

/// Standard QEMU SLIRP guest & gateway IPv4 configuration (used as deterministic fallback).
pub const LOCAL_IPV4: [u8; 4] = [10, 0, 2, 15];
pub const GATEWAY_IPV4: [u8; 4] = [10, 0, 2, 2];
pub const FALLBACK_SLIRP_GW_MAC: [u8; 6] = [0x52, 0x55, 0x0A, 0x00, 0x02, 0x02];

static CURRENT_IPV4: SpinLock<[u8; 4]> = SpinLock::new(LOCAL_IPV4);
static CURRENT_GATEWAY_IPV4: SpinLock<[u8; 4]> = SpinLock::new(GATEWAY_IPV4);

/// Returns the currently assigned IPv4 address (from DHCP lease or fallback).
pub fn current_ipv4() -> [u8; 4] {
    *CURRENT_IPV4.lock()
}

/// Returns the currently configured gateway IPv4 address.
pub fn current_gateway_ipv4() -> [u8; 4] {
    *CURRENT_GATEWAY_IPV4.lock()
}

/// Updates the active network interface IP and Gateway configuration.
pub fn set_assigned_ip(ip: [u8; 4], gw: [u8; 4]) {
    *CURRENT_IPV4.lock() = ip;
    *CURRENT_GATEWAY_IPV4.lock() = gw;
}

static BG_NET_POLL_HOOK: SpinLock<Option<fn()>> = SpinLock::new(None);

/// Registers a non-blocking background network service poll callback invoked from the AP worker loop (outside IRQ).
pub fn register_bg_net_poll_hook(cb: fn()) {
    *BG_NET_POLL_HOOK.lock() = Some(cb);
}

/// Lightweight non-blocking network polling helper invoked from the AP worker loop (outside IRQ).
pub fn poll_network_and_remote_console() {
    remote_shell::poll_remote_console_tcp();
    let hook = BG_NET_POLL_HOOK.try_lock().and_then(|g| *g);
    if let Some(cb) = hook {
        cb();
    }
}

static ICMP_SEQ: AtomicU16 = AtomicU16::new(1);

pub struct NetStackState {
    pub nic: E1000Controller,
    pub gateway_mac: Option<[u8; 6]>,
    pub last_rtt_us: u16,
    pub live_wire_rx: bool,
}

impl NetStackState {
    const fn new() -> Self {
        Self {
            nic: E1000Controller::uninitialized(),
            gateway_mac: None,
            last_rtt_us: 0,
            live_wire_rx: false,
        }
    }
}

pub static NET_STACK: SpinLock<NetStackState> = SpinLock::new(NetStackState::new());

/// Complete diagnostic result of an `ARP + IPv4 ICMP Echo` exchange over `/dev/net/eth0`.
#[derive(Debug, Clone, Copy)]
pub struct NetPingResult {
    pub hw: NicHardwareSnapshot,
    pub local_ip: [u8; 4],
    pub gateway_ip: [u8; 4],
    pub gateway_mac: [u8; 6],
    pub arp_resolved: bool,
    pub icmp_reply_ok: bool,
    pub live_wire_rx: bool,
    pub seq: u16,
    pub rtt_us: u16,
}

impl NetPingResult {
    pub const fn empty() -> Self {
        Self {
            hw: NicHardwareSnapshot {
                present: false,
                pci_bus: 0,
                pci_dev: 0,
                pci_fn: 0,
                vendor_id: 0,
                device_id: 0,
                pci_cmd: 0,
                mmio_phys: 0,
                rx_ring_phys: 0,
                tx_ring_phys: 0,
                mac: [0; 6],
                link_up: false,
                tx_packets: 0,
                rx_packets: 0,
                tx_bytes: 0,
                rx_bytes: 0,
            },
            local_ip: [0; 4],
            gateway_ip: [0; 4],
            gateway_mac: [0; 6],
            arp_resolved: false,
            icmp_reply_ok: false,
            live_wire_rx: false,
            seq: 0,
            rtt_us: 0,
        }
    }

    #[inline]
    pub const fn pack_u64(&self) -> u64 {
        let flags = (self.hw.link_up as u64)
            | ((self.arp_resolved as u64) << 1)
            | ((self.icmp_reply_ok as u64) << 2)
            | ((self.live_wire_rx as u64) << 3);
        let tx16 = (self.hw.tx_packets & 0xFFFF) << 24;
        let rx16 = (self.hw.rx_packets & 0xFFFF) << 40;
        flags | ((self.rtt_us as u64) << 8) | tx16 | rx16
    }
}

/// Initializes the PCIe Intel `e1000`/`e1000e` controller and negotiates DHCP lease if present.
pub fn init() {
    let linked = {
        let mut guard = NET_STACK.lock();
        guard.nic.init_hardware();
        guard.nic.is_present()
    };
    if linked {
        dhcp::discover_and_configure();
    }
}

/// Returns a telemetry snapshot of the hardware NIC registers.
pub fn snapshot() -> NicHardwareSnapshot {
    let guard = NET_STACK.lock();
    guard.nic.snapshot()
}

/// Executes a real hardware `ARP Request -> ARP Reply` resolution and `ICMP Echo Request -> Echo Reply`
/// round-trip against the QEMU Virtual Gateway (`10.0.2.2`).
pub fn ping_gateway() -> NetPingResult {
    let mut guard = NET_STACK.lock();
    if !guard.nic.is_present() {
        return NetPingResult::empty();
    }

    let src_mac = guard.nic.mac_address();
    let src_ip = current_ipv4();
    let gw_ip = current_gateway_ipv4();

    // 1. Resolve Gateway MAC via ARP Request if not yet cached
    let (arp_ok, gw_mac) = match guard.gateway_mac {
        Some(mac) => (true, mac),
        None => {
            let arp_frame = build_arp_frame(
                src_mac,
                src_ip,
                [0xFF; 6],
                [0x00; 6],
                gw_ip,
                1,
            );
            let _ = guard.nic.transmit_frame(&arp_frame);

            let mut rx_buf = [0u8; PKT_BUF_SIZE];
            let mut resolved = None;
            let arp_dl = crate::time::tsc_deadline_ms(150);
            while !crate::time::tsc_expired(arp_dl) {
                if let Some(len) = guard.nic.receive_frame(&mut rx_buf) {
                    guard.live_wire_rx = true;
                    if let Some(mac) =
                        inspect_and_answer_arp(&mut guard.nic, &rx_buf[..len], src_mac)
                    {
                        resolved = Some(mac);
                        break;
                    }
                }
                core::hint::spin_loop();
            }

            (resolved.is_some(), resolved.unwrap_or(FALLBACK_SLIRP_GW_MAC))
        }
    };
    if arp_ok {
        guard.gateway_mac = Some(gw_mac);
    }

    // 2. Transmit ICMP Echo Request
    let seq = ICMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let icmp_payload = b"FipSOs_E1000E_PING_OK_2026";
    let mut tx_buf = [0u8; 128];
    let tx_len = build_icmp_echo_frame(
        src_mac,
        gw_mac,
        src_ip,
        gw_ip,
        seq,
        icmp_payload,
        &mut tx_buf,
    );

    let start_tsc = read_tsc();
    let _ = guard.nic.transmit_frame(&tx_buf[..tx_len]);

    // 3. Receive wire frames until ICMP Echo Reply arrives
    let mut rx_buf = [0u8; PKT_BUF_SIZE];
    let mut success = false;
    let icmp_dl = crate::time::tsc_deadline_ms(250);
    while !crate::time::tsc_expired(icmp_dl) {
        if let Some(len) = guard.nic.receive_frame(&mut rx_buf) {
            guard.live_wire_rx = true;
            if let Some(updated_gw) =
                inspect_and_answer_arp(&mut guard.nic, &rx_buf[..len], src_mac)
            {
                guard.gateway_mac = Some(updated_gw);
            }
            if is_icmp_echo_reply(&rx_buf[..len], seq) {
                success = true;
                break;
            }
        }
        core::hint::spin_loop();
    }

    let end_tsc = read_tsc();
    let elapsed_tsc = end_tsc.saturating_sub(start_tsc);
    let cycles_per_us = (crate::time::tsc_hz() / 1_000_000).max(1);
    let rtt_us = (elapsed_tsc / cycles_per_us).min(65535) as u16;
    guard.last_rtt_us = rtt_us;

    let snap = guard.nic.snapshot();
    NetPingResult {
        hw: snap,
        local_ip: src_ip,
        gateway_ip: gw_ip,
        gateway_mac: gw_mac,
        arp_resolved: arp_ok,
        icmp_reply_ok: success,
        live_wire_rx: guard.live_wire_rx,
        seq,
        rtt_us,
    }
}

fn parse_ipv4_literal(s: &str) -> Option<[u8; 4]> {
    let mut octets = [0u8; 4];
    let mut idx = 0usize;
    for part in s.split('.') {
        if idx >= 4 {
            return None;
        }
        octets[idx] = part.parse::<u8>().ok()?;
        idx += 1;
    }
    if idx == 4 { Some(octets) } else { None }
}

/// Downloads a software package or remote note via HTTP GET over `/dev/net/eth0`.
pub fn http_fetch_package(resource_path: &str) -> (NetPingResult, Vec<u8>) {
    let gw = current_gateway_ipv4();
    let url = format!("http://{}.{}.{}.{}{}", gw[0], gw[1], gw[2], gw[3], resource_path);
    http_fetch_url(&url)
}

/// Performs an HTTP/1.0 GET request over a real RFC 793 TCP 3-way handshake connection.
pub fn http_fetch_url(uri: &str) -> (NetPingResult, Vec<u8>) {
    let ping_info = ping_gateway();
    if !ping_info.icmp_reply_ok {
        return (ping_info, Vec::new());
    }

    let gw_ip = current_gateway_ipv4();
    let (authority, path) = if let Some(pos) = uri.find("://") {
        let after_scheme = &uri[pos + 3..];
        if let Some(slash) = after_scheme.find('/') {
            (&after_scheme[..slash], &after_scheme[slash..])
        } else {
            (after_scheme, "/")
        }
    } else {
        ("", uri)
    };

    let (host_str, dst_port) = if let Some(colon) = authority.rfind(':') {
        let p = authority[colon + 1..].parse::<u16>().unwrap_or(80);
        (&authority[..colon], p)
    } else {
        (authority, 80u16)
    };

    let dst_ip = parse_ipv4_literal(host_str).unwrap_or(gw_ip);
    let host_header = if host_str.is_empty() {
        format!("{}.{}.{}.{}", dst_ip[0], dst_ip[1], dst_ip[2], dst_ip[3])
    } else {
        alloc::string::String::from(host_str)
    };

    let Ok(sock_id) = tcp_client::create_socket() else {
        return (ping_info, Vec::new());
    };
    if tcp_client::connect(sock_id, dst_ip, dst_port).is_err() {
        let _ = tcp_client::close(sock_id);
        return (ping_info, Vec::new());
    }

    let req = format!(
        "GET {} HTTP/1.0\r\nHost: {}\r\nUser-Agent: FipSOs/6.1\r\nConnection: close\r\n\r\n",
        path, host_header
    );
    if tcp_client::send(sock_id, req.as_bytes()).is_err() {
        let _ = tcp_client::close(sock_id);
        return (ping_info, Vec::new());
    }

    let mut raw_resp = Vec::new();
    let mut chunk = [0u8; 1024];
    for _ in 0..64 {
        match tcp_client::recv(sock_id, &mut chunk) {
            Ok(0) => break,
            Ok(n) => raw_resp.extend_from_slice(&chunk[..n]),
            Err(_) => break,
        }
    }
    let _ = tcp_client::close(sock_id);

    if let Some(sep) = raw_resp.windows(4).position(|w| w == b"\r\n\r\n") {
        (ping_info, raw_resp[sep + 4..].to_vec())
    } else {
        (ping_info, raw_resp)
    }
}

#[inline]
pub fn read_tsc() -> u64 {
    #[cfg(target_arch = "x86_64")]
    {
        let lo: u32;
        let hi: u32;
        // SAFETY: `rdtsc` is a side-effect-free timestamp read valid in Ring 0.
        unsafe {
            core::arch::asm!(
                "rdtsc",
                out("eax") lo,
                out("edx") hi,
                options(nomem, nostack, preserves_flags)
            );
        }
        ((hi as u64) << 32) | (lo as u64)
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        0
    }
}
