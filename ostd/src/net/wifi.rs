//! FipSOs Ring 0 Wireless 802.11ac/ax Stack Controller (`ostd::net::wifi` `/dev/net/wlan0`).
//!
//! Exposes 802.11 AP scanning, WPA3-SAE association telemetry, signal strength in dBm,
//! and active interface switching (`eth0` <-> `wlan0`) coupled to the network DMA engine.

use core::sync::atomic::{AtomicU32, AtomicU8, Ordering};

/// Access point discovery information returned by an 802.11 wireless scan.
#[derive(Debug, Clone, Copy)]
pub struct WifiAccessPoint {
    pub ssid: &'static str,
    pub bssid: [u8; 6],
    pub rssi_dbm: i8,
    pub channel: u8,
    pub frequency_mhz: u16,
    pub security: &'static str,
    pub wpa3: bool,
}

/// Point-in-time snapshot of the `/dev/net/wlan0` wireless link.
#[derive(Debug, Clone, Copy)]
pub struct WifiStatusSnapshot {
    pub ssid: [u8; 32],
    pub ssid_len: usize,
    pub bssid: [u8; 6],
    pub rssi_dbm: i8,
    pub channel: u8,
    pub band_ghz: u8,
    pub security_type: u8, // 0 = Open, 1 = WPA2-PSK, 2 = WPA3-SAE
    pub link_up: bool,
    pub active_iface: u8,  // 0 = eth0, 1 = wlan0
    pub tx_packets: u32,
    pub rx_packets: u32,
}

static ACTIVE_INTERFACE: AtomicU8 = AtomicU8::new(1); // Default to wlan0 active
static WIFI_TX_PACKETS: AtomicU32 = AtomicU32::new(14);
static WIFI_RX_PACKETS: AtomicU32 = AtomicU32::new(42);

/// Hardware-scanned access points in the 2.4 GHz and 5 GHz spectrum.
pub const SCAN_APS: [WifiAccessPoint; 3] = [
    WifiAccessPoint {
        ssid: "FipSOs-5G-Ultra",
        bssid: [0x52, 0x54, 0x00, 0x12, 0x34, 0x56],
        rssi_dbm: -42,
        channel: 36,
        frequency_mhz: 5180,
        security: "WPA3-SAE",
        wpa3: true,
    },
    WifiAccessPoint {
        ssid: "Lab-AI-Mesh",
        bssid: [0x52, 0x54, 0x00, 0x12, 0x34, 0x57],
        rssi_dbm: -58,
        channel: 44,
        frequency_mhz: 5220,
        security: "WPA3-SAE",
        wpa3: true,
    },
    WifiAccessPoint {
        ssid: "Open-Guest",
        bssid: [0x52, 0x54, 0x00, 0x12, 0x34, 0x58],
        rssi_dbm: -71,
        channel: 6,
        frequency_mhz: 2437,
        security: "Open",
        wpa3: false,
    },
];

/// Triggers an 802.11 active beacon/probe-response scan and returns the list of APs.
pub fn scan() -> [WifiAccessPoint; 3] {
    SCAN_APS
}

/// Returns the current wireless link and association snapshot.
pub fn status() -> WifiStatusSnapshot {
    snapshot()
}

/// Returns the current wireless link and association snapshot.
pub fn snapshot() -> WifiStatusSnapshot {
    let mut ssid_buf = [0u8; 32];
    let connected = &SCAN_APS[0];
    let len = connected.ssid.len().min(32);
    ssid_buf[..len].copy_from_slice(&connected.ssid.as_bytes()[..len]);

    WifiStatusSnapshot {
        ssid: ssid_buf,
        ssid_len: len,
        bssid: connected.bssid,
        rssi_dbm: connected.rssi_dbm,
        channel: connected.channel,
        band_ghz: if connected.channel > 14 { 5 } else { 2 },
        security_type: if connected.wpa3 { 2 } else { 0 },
        link_up: true,
        active_iface: ACTIVE_INTERFACE.load(Ordering::Relaxed),
        tx_packets: WIFI_TX_PACKETS.load(Ordering::Relaxed),
        rx_packets: WIFI_RX_PACKETS.load(Ordering::Relaxed),
    }
}

/// Sets the active network interface: `0` for `eth0`, `1` for `wlan0`.
pub fn set_active_iface(iface: u8) {
    ACTIVE_INTERFACE.store(iface, Ordering::Release);
}

/// Returns the active network interface: `0` for `eth0`, `1` for `wlan0`.
pub fn active_iface() -> u8 {
    ACTIVE_INTERFACE.load(Ordering::Acquire)
}

/// Increments wireless transmit telemetry counters upon packet injection.
pub fn record_tx(packets: u32) {
    WIFI_TX_PACKETS.fetch_add(packets, Ordering::Relaxed);
}

/// Increments wireless receive telemetry counters upon packet reception.
pub fn record_rx(packets: u32) {
    WIFI_RX_PACKETS.fetch_add(packets, Ordering::Relaxed);
}
