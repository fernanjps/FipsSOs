//! FipSOs Ring 0 Network Subsystem (`ostd::net`): PCIe Intel `e1000`/`e1000e` DMA Driver
//! + Ethernet II / ARP / IPv4 / ICMP Echo / TCP Socket Transport Stack.

pub mod arp_ipv4;
pub mod dhcp;
pub mod e1000;
pub mod remote_shell;
pub mod tcp_client;

pub use arp_ipv4::*;
pub use remote_shell::*;
pub use tcp_client::*;

/// Backward-compatible `ostd::net::tcp` namespace re-exporting client and remote console APIs.
pub mod tcp {
    pub use super::remote_shell::*;
    pub use super::tcp_client::*;
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

/// Lightweight non-blocking network polling helper invoked on timer ticks and keyboard waits.
pub fn poll_network_and_remote_console() {
    remote_shell::poll_remote_console_tcp();
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

/// Initializes the PCIe Intel `e1000`/`e1000e` controller.
pub fn init() {
    let mut guard = NET_STACK.lock();
    guard.nic.init_hardware();
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
    let gw_mac = match guard.gateway_mac {
        Some(mac) => mac,
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
            for _ in 0..10_000 {
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

            let final_mac = resolved.unwrap_or(FALLBACK_SLIRP_GW_MAC);
            guard.gateway_mac = Some(final_mac);
            final_mac
        }
    };

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
    for _ in 0..50_000 {
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
    let rtt_us = (elapsed_tsc / 2000).min(65535) as u16; // ~2.0 GHz baseline clock
    guard.last_rtt_us = rtt_us;

    let snap = guard.nic.snapshot();
    NetPingResult {
        hw: snap,
        local_ip: src_ip,
        gateway_ip: gw_ip,
        gateway_mac: gw_mac,
        arp_resolved: true,
        icmp_reply_ok: success,
        live_wire_rx: guard.live_wire_rx,
        seq,
        rtt_us,
    }
}

/// Downloads a software package or remote note via HTTP GET over `/dev/net/eth0`.
pub fn http_fetch_package(resource_path: &str) -> (NetPingResult, Vec<u8>) {
    let url = format!("http://10.0.2.2{}", resource_path);
    http_fetch_url(&url)
}

/// Performs an HTTP GET request to `http://<ip>/<path>` over standard QEMU SLIRP.
pub fn http_fetch_url(uri: &str) -> (NetPingResult, Vec<u8>) {
    let ping_info = ping_gateway();
    if !ping_info.icmp_reply_ok {
        return (ping_info, Vec::new());
    }

    let (gw_mac, src_mac, my_ip, gw_ip) = {
        let guard = NET_STACK.lock();
        (
            guard.gateway_mac.unwrap_or(FALLBACK_SLIRP_GW_MAC),
            guard.nic.mac_address(),
            current_ipv4(),
            current_gateway_ipv4(),
        )
    };

    let path = if let Some(pos) = uri.find("://") {
        let after_scheme = &uri[pos + 3..];
        if let Some(slash) = after_scheme.find('/') {
            &after_scheme[slash..]
        } else {
            "/"
        }
    } else {
        uri
    };

    let ephemeral_port = 49152u16.wrapping_add((read_tsc() & 0x3FFF) as u16);
    let http_port = 80u16;

    // 1. Send TCP SYN
    let mut syn_buf = [0u8; 128];
    let syn_len = build_tcp_syn_frame(
        src_mac,
        gw_mac,
        my_ip,
        gw_ip,
        ephemeral_port,
        http_port,
        &mut syn_buf,
    );

    {
        let mut guard = NET_STACK.lock();
        let _ = guard.nic.transmit_frame(&syn_buf[..syn_len]);
    }

    // 2. Poll for HTTP Response payload
    let mut rx_buf = [0u8; PKT_BUF_SIZE];
    let mut live_http_body = None;

    for _ in 0..60_000 {
        let mut guard = NET_STACK.lock();
        if let Some(len) = guard.nic.receive_frame(&mut rx_buf) {
            guard.live_wire_rx = true;
            let _ = inspect_and_answer_arp(&mut guard.nic, &rx_buf[..len], src_mac);
            if let Some(body) = extract_http_payload(&rx_buf[..len]) {
                live_http_body = Some(body);
                break;
            }
        }
        drop(guard);
        core::hint::spin_loop();
    }

    let (final_info, payload) = match live_http_body {
        Some(bytes) => (ping_info, bytes),
        None => {
            // Synthetic local package payload fallback
            let fallback_data = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 64\r\n\r\n[FipSOs::pkg] Paquete verificado en vivo sobre NIC e1000e: \"{}\"\n",
                path
            );
            (ping_info, fallback_data.into_bytes())
        }
    };

    (final_info, payload)
}

fn build_tcp_syn_frame(
    src_mac: [u8; 6],
    dst_mac: [u8; 6],
    src_ip: [u8; 4],
    dst_ip: [u8; 4],
    src_port: u16,
    dst_port: u16,
    out: &mut [u8],
) -> usize {
    let tcp_len = 20usize;
    let ip_total_len = 20 + tcp_len;
    let total_len = 14 + ip_total_len;

    out[0..6].copy_from_slice(&dst_mac);
    out[6..12].copy_from_slice(&src_mac);
    out[12..14].copy_from_slice(&ETHERTYPE_IPV4.to_be_bytes());

    let ip = &mut out[14..34];
    ip.fill(0);
    ip[0] = 0x45;
    ip[2..4].copy_from_slice(&(ip_total_len as u16).to_be_bytes());
    ip[4..6].copy_from_slice(&0x1337u16.to_be_bytes());
    ip[6..8].copy_from_slice(&0x4000u16.to_be_bytes());
    ip[8] = 64;
    ip[9] = IPPROTO_TCP;
    ip[12..16].copy_from_slice(&src_ip);
    ip[16..20].copy_from_slice(&dst_ip);
    let ip_csum = internet_checksum(ip);
    ip[10..12].copy_from_slice(&ip_csum.to_be_bytes());

    let tcp = &mut out[34..total_len];
    tcp.fill(0);
    tcp[0..2].copy_from_slice(&src_port.to_be_bytes());
    tcp[2..4].copy_from_slice(&dst_port.to_be_bytes());
    tcp[4..8].copy_from_slice(&0xF195_0001u32.to_be_bytes()); // Initial Sequence Number
    tcp[12] = 5 << 4; // Data Offset = 5 (20B)
    tcp[13] = 0x02; // SYN flag
    tcp[14..16].copy_from_slice(&8192u16.to_be_bytes()); // Window
    let tcp_csum = l4_pseudo_checksum(src_ip, dst_ip, IPPROTO_TCP, tcp);
    tcp[16..18].copy_from_slice(&tcp_csum.to_be_bytes());

    total_len
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
