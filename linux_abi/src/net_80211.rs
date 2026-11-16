#![forbid(unsafe_code)]

//! IEEE 802.11 Framing, WPA2-PSK Key Derivation (`PBKDF2-HMAC-SHA1`, `PRF-PTK`),
//! and `AES-128-CCMP` Authenticated Encryption (`#![forbid(unsafe_code)]`).

use alloc::format;
use alloc::vec::Vec;

const SBOX: [u8; 256] = [
    0x63,0x7c,0x77,0x7b,0xf2,0x6b,0x6f,0xc5,0x30,0x01,0x67,0x2b,0xfe,0xd7,0xab,0x76,
    0xca,0x82,0xc9,0x7d,0xfa,0x59,0x47,0xf0,0xad,0xd4,0xa2,0xaf,0x9c,0xa4,0x72,0xc0,
    0xb7,0xfd,0x93,0x26,0x36,0x3f,0xf7,0xcc,0x34,0xa5,0xe5,0xf1,0x71,0xd8,0x31,0x15,
    0x04,0xc7,0x23,0xc3,0x18,0x96,0x05,0x9a,0x07,0x12,0x80,0xe2,0xeb,0x27,0xb2,0x75,
    0x09,0x83,0x2c,0x1a,0x1b,0x6e,0x5a,0xa0,0x52,0x3b,0xd6,0xb3,0x29,0xe3,0x2f,0x84,
    0x53,0xd1,0x00,0xed,0x20,0xfc,0xb1,0x5b,0x6a,0xcb,0xbe,0x39,0x4a,0x4c,0x58,0xcf,
    0xd0,0xef,0xaa,0xfb,0x43,0x4d,0x33,0x85,0x45,0xf9,0x02,0x7f,0x50,0x3c,0x9f,0xa8,
    0x51,0xa3,0x40,0x8f,0x92,0x9d,0x38,0xf5,0xbc,0xb6,0xda,0x21,0x10,0xff,0xf3,0xd2,
    0xcd,0x0c,0x13,0xec,0x5f,0x97,0x44,0x17,0xc4,0xa7,0x7e,0x3d,0x64,0x5d,0x19,0x73,
    0x60,0x81,0x4f,0xdc,0x22,0x2a,0x90,0x88,0x46,0xee,0xb8,0x14,0xde,0x5e,0x0b,0xdb,
    0xe0,0x32,0x3a,0x0a,0x49,0x06,0x24,0x5c,0xc2,0xd3,0xac,0x62,0x91,0x95,0xe4,0x79,
    0xe7,0xc8,0x37,0x6d,0x8d,0xd5,0x4e,0xa9,0x6c,0x56,0xf4,0xea,0x65,0x7a,0xae,0x08,
    0xba,0x78,0x25,0x2e,0x1c,0xa6,0xb4,0xc6,0xe8,0xdd,0x74,0x1f,0x4b,0xbd,0x8b,0x8a,
    0x70,0x3e,0xb5,0x66,0x48,0x03,0xf6,0x0e,0x61,0x35,0x57,0xb9,0x86,0xc1,0x1d,0x9e,
    0xe1,0xf8,0x98,0x11,0x69,0xd9,0x8e,0x94,0x9b,0x1e,0x87,0xe9,0xce,0x55,0x28,0xdf,
    0x8c,0xa1,0x89,0x0d,0xbf,0xe6,0x42,0x68,0x41,0x99,0x2d,0x0f,0xb0,0x54,0xbb,0x16,
];

const RCON: [u8; 10] = [0x01, 0x02, 0x04, 0x08, 0x10, 0x20, 0x40, 0x80, 0x1b, 0x36];

/// Computes SHA-1 digest (20 bytes) per FIPS 180-4.
pub fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h = [0x6745_2301u32, 0xEFCD_AB89, 0x98BA_DCFE, 0x1032_5476, 0xC3D2_E1F0];
    let bit_len = (data.len() as u64) * 8;
    let mut msg = Vec::with_capacity(data.len() + 72);
    msg.extend_from_slice(data);
    msg.push(0x80);
    while (msg.len() % 64) != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in msg.chunks_exact(64) {
        let mut w = [0u32; 80];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([chunk[i * 4], chunk[i * 4 + 1], chunk[i * 4 + 2], chunk[i * 4 + 3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = h;
        for (i, &wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | ((!b) & d), 0x5A82_7999u32),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1u32),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDCu32),
                _ => (b ^ c ^ d, 0xCA62_C1D6u32),
            };
            let tmp = a.rotate_left(5).wrapping_add(f).wrapping_add(e).wrapping_add(k).wrapping_add(wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = tmp;
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
    }
    let mut out = [0u8; 20];
    for (i, word) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

/// Computes HMAC-SHA1 (20 bytes) per RFC 2104.
pub fn hmac_sha1(key: &[u8], data: &[u8]) -> [u8; 20] {
    let mut k_block = [0u8; 64];
    if key.len() > 64 {
        let kh = sha1(key);
        k_block[..20].copy_from_slice(&kh);
    } else {
        k_block[..key.len()].copy_from_slice(key);
    }
    let mut inner = Vec::with_capacity(64 + data.len());
    for &b in &k_block {
        inner.push(b ^ 0x36);
    }
    inner.extend_from_slice(data);
    let ih = sha1(&inner);

    let mut outer = Vec::with_capacity(64 + 20);
    for &b in &k_block {
        outer.push(b ^ 0x5C);
    }
    outer.extend_from_slice(&ih);
    sha1(&outer)
}

/// Derives a 256-bit WPA2 PMK using PBKDF2-HMAC-SHA1 (4096 iterations per IEEE 802.11i).
pub fn pbkdf2_hmac_sha1(passphrase: &[u8], ssid: &[u8], iterations: u32) -> [u8; 32] {
    let mut pmk = [0u8; 32];
    for block_idx in 1u32..=2 {
        let mut salt = Vec::with_capacity(ssid.len() + 4);
        salt.extend_from_slice(ssid);
        salt.extend_from_slice(&block_idx.to_be_bytes());
        let mut u = hmac_sha1(passphrase, &salt);
        let mut t = u;
        for _ in 1..iterations {
            u = hmac_sha1(passphrase, &u);
            for (dst, src) in t.iter_mut().zip(u.iter()) {
                *dst ^= *src;
            }
        }
        let off = ((block_idx - 1) * 20) as usize;
        let copy_len = (32 - off).min(20);
        pmk[off..off + copy_len].copy_from_slice(&t[..copy_len]);
    }
    pmk
}

/// Derives the 48-byte PTK (KCK[16] + KEK[16] + TK[16]) via WPA2 PRF-384.
pub fn derive_ptk(
    pmk: &[u8; 32],
    aa: &[u8; 6],
    spa: &[u8; 6],
    anonce: &[u8; 32],
    snonce: &[u8; 32],
) -> [u8; 48] {
    let mut data = [0u8; 76];
    let (m1, m2) = if aa < spa { (aa, spa) } else { (spa, aa) };
    let (n1, n2) = if anonce < snonce { (anonce, snonce) } else { (snonce, anonce) };
    data[0..6].copy_from_slice(m1);
    data[6..12].copy_from_slice(m2);
    data[12..44].copy_from_slice(n1);
    data[44..76].copy_from_slice(n2);

    let prefix = b"Pairwise key expansion\0";
    let mut ptk = [0u8; 48];
    for i in 0u8..3 {
        let mut msg = Vec::with_capacity(prefix.len() + data.len() + 1);
        msg.extend_from_slice(prefix);
        msg.extend_from_slice(&data);
        msg.push(i);
        let digest = hmac_sha1(pmk, &msg);
        let off = (i as usize) * 20;
        let take = (48 - off).min(20);
        ptk[off..off + take].copy_from_slice(&digest[..take]);
    }
    ptk
}

#[inline]
fn xtime(x: u8) -> u8 {
    (x << 1) ^ (((x >> 7) & 1) * 0x1b)
}

fn aes_expand_key(key: &[u8; 16]) -> [[u8; 16]; 11] {
    let mut rk = [[0u8; 16]; 11];
    rk[0] = *key;
    for r in 0..10 {
        let prev = rk[r];
        let mut next = [0u8; 16];
        next[0] = prev[0] ^ SBOX[prev[13] as usize] ^ RCON[r];
        next[1] = prev[1] ^ SBOX[prev[14] as usize];
        next[2] = prev[2] ^ SBOX[prev[15] as usize];
        next[3] = prev[3] ^ SBOX[prev[12] as usize];
        for i in 4..16 {
            next[i] = prev[i] ^ next[i - 4];
        }
        rk[r + 1] = next;
    }
    rk
}

/// Encrypts a single 16-byte block with AES-128 (FIPS 197).
pub fn aes128_encrypt_block(key: &[u8; 16], block: &[u8; 16]) -> [u8; 16] {
    let rk = aes_expand_key(key);
    let mut s = *block;
    for i in 0..16 {
        s[i] ^= rk[0][i];
    }
    for round in 1..=10 {
        for b in s.iter_mut() {
            *b = SBOX[*b as usize];
        }
        // ShiftRows
        let t = s;
        s[1] = t[5]; s[5] = t[9]; s[9] = t[13]; s[13] = t[1];
        s[2] = t[10]; s[6] = t[14]; s[10] = t[2]; s[14] = t[6];
        s[3] = t[15]; s[7] = t[3]; s[11] = t[7]; s[15] = t[11];
        if round < 10 {
            for col in 0..4 {
                let c = col * 4;
                let (a0, a1, a2, a3) = (s[c], s[c + 1], s[c + 2], s[c + 3]);
                let xor_all = a0 ^ a1 ^ a2 ^ a3;
                s[c] ^= xor_all ^ xtime(a0 ^ a1);
                s[c + 1] ^= xor_all ^ xtime(a1 ^ a2);
                s[c + 2] ^= xor_all ^ xtime(a2 ^ a3);
                s[c + 3] ^= xor_all ^ xtime(a3 ^ a0);
            }
        }
        for i in 0..16 {
            s[i] ^= rk[round][i];
        }
    }
    s
}

fn ccmp_nonce(a2: &[u8; 6], pn: u64, priority: u8) -> [u8; 13] {
    let mut n = [0u8; 13];
    n[0] = priority & 0x0F;
    n[1..7].copy_from_slice(a2);
    let pn_be = pn.to_be_bytes();
    n[7..13].copy_from_slice(&pn_be[2..8]);
    n
}

fn ccmp_cbc_mac(tk: &[u8; 16], nonce: &[u8; 13], aad: &[u8], pt: &[u8]) -> [u8; 8] {
    let mut b0 = [0u8; 16];
    b0[0] = 0x59; // Adata=1, M=8 ((8-2)/2=3 -> 0x18), L=2 (L-1=1 -> 0x01) => 0x40|0x18|0x01
    b0[1..14].copy_from_slice(nonce);
    b0[14] = ((pt.len() >> 8) & 0xFF) as u8;
    b0[15] = (pt.len() & 0xFF) as u8;
    let mut mac = aes128_encrypt_block(tk, &b0);

    let mut aad_blk = Vec::with_capacity(2 + aad.len() + 16);
    aad_blk.push(((aad.len() >> 8) & 0xFF) as u8);
    aad_blk.push((aad.len() & 0xFF) as u8);
    aad_blk.extend_from_slice(aad);
    while (aad_blk.len() % 16) != 0 {
        aad_blk.push(0);
    }
    for blk in aad_blk.chunks_exact(16) {
        for i in 0..16 {
            mac[i] ^= blk[i];
        }
        mac = aes128_encrypt_block(tk, &mac);
    }
    for blk in pt.chunks(16) {
        for (i, &b) in blk.iter().enumerate() {
            mac[i] ^= b;
        }
        mac = aes128_encrypt_block(tk, &mac);
    }
    let mut mic = [0u8; 8];
    mic.copy_from_slice(&mac[..8]);
    mic
}

/// Encrypts and authenticates an 802.11 payload using AES-128-CCMP (returns `ciphertext || 8B MIC`).
pub fn ccmp_encrypt(tk: &[u8; 16], a2: &[u8; 6], pn: u64, aad: &[u8], pt: &[u8]) -> Vec<u8> {
    let nonce = ccmp_nonce(a2, pn, 0);
    let mic = ccmp_cbc_mac(tk, &nonce, aad, pt);
    let mut out = Vec::with_capacity(pt.len() + 8);
    let mut ctr = [0u8; 16];
    ctr[0] = 0x01; // L-1 = 1
    ctr[1..14].copy_from_slice(&nonce);

    let s0 = aes128_encrypt_block(tk, &ctr);
    for (idx, chunk) in pt.chunks(16).enumerate() {
        let c_val = (idx + 1) as u16;
        ctr[14] = (c_val >> 8) as u8;
        ctr[15] = (c_val & 0xFF) as u8;
        let ks = aes128_encrypt_block(tk, &ctr);
        for (i, &b) in chunk.iter().enumerate() {
            out.push(b ^ ks[i]);
        }
    }
    for i in 0..8 {
        out.push(mic[i] ^ s0[i]);
    }
    out
}

/// Decrypts and verifies an AES-128-CCMP payload (`ciphertext || 8B MIC`). Returns `Err(-74)` (`-EBADMSG`) on MIC mismatch.
pub fn ccmp_decrypt(tk: &[u8; 16], a2: &[u8; 6], pn: u64, aad: &[u8], ct_mic: &[u8]) -> Result<Vec<u8>, i64> {
    if ct_mic.len() < 8 {
        return Err(-22);
    }
    let ct_len = ct_mic.len() - 8;
    let nonce = ccmp_nonce(a2, pn, 0);
    let mut ctr = [0u8; 16];
    ctr[0] = 0x01;
    ctr[1..14].copy_from_slice(&nonce);
    let s0 = aes128_encrypt_block(tk, &ctr);

    let mut pt = Vec::with_capacity(ct_len);
    for (idx, chunk) in ct_mic[..ct_len].chunks(16).enumerate() {
        let c_val = (idx + 1) as u16;
        ctr[14] = (c_val >> 8) as u8;
        ctr[15] = (c_val & 0xFF) as u8;
        let ks = aes128_encrypt_block(tk, &ctr);
        for (i, &b) in chunk.iter().enumerate() {
            pt.push(b ^ ks[i]);
        }
    }
    let calc_mic = ccmp_cbc_mac(tk, &nonce, aad, &pt);
    for i in 0..8 {
        if (ct_mic[ct_len + i] ^ s0[i]) != calc_mic[i] {
            return Err(-74); // -EBADMSG
        }
    }
    Ok(pt)
}

/// Builds an IEEE 802.11 Open-System Authentication Management frame.
pub fn build_auth_frame(da: &[u8; 6], sa: &[u8; 6], bssid: &[u8; 6], seq: u16) -> Vec<u8> {
    let mut f = Vec::with_capacity(30);
    f.extend_from_slice(&0x00B0u16.to_le_bytes()); // Type=Mgmt(0), Subtype=Auth(11)
    f.extend_from_slice(&[0x00, 0x00]); // Duration
    f.extend_from_slice(da);
    f.extend_from_slice(sa);
    f.extend_from_slice(bssid);
    f.extend_from_slice(&(seq << 4).to_le_bytes());
    f.extend_from_slice(&0u16.to_le_bytes()); // Auth Alg: Open System
    f.extend_from_slice(&1u16.to_le_bytes()); // Seq 1
    f.extend_from_slice(&0u16.to_le_bytes()); // Status: Success
    f
}

/// Builds an IEEE 802.11 Association Request frame carrying SSID IE.
pub fn build_assoc_req_frame(da: &[u8; 6], sa: &[u8; 6], bssid: &[u8; 6], ssid: &[u8]) -> Vec<u8> {
    let mut f = Vec::with_capacity(30 + ssid.len());
    f.extend_from_slice(&0x0000u16.to_le_bytes()); // Type=Mgmt(0), Subtype=AssocReq(0)
    f.extend_from_slice(&[0x00, 0x00]);
    f.extend_from_slice(da);
    f.extend_from_slice(sa);
    f.extend_from_slice(bssid);
    f.extend_from_slice(&0x0010u16.to_le_bytes());
    f.extend_from_slice(&0x0411u16.to_le_bytes()); // Capability Info (ESS + Privacy)
    f.extend_from_slice(&10u16.to_le_bytes()); // Listen Interval
    f.push(0); // SSID Tag
    f.push(ssid.len().min(32) as u8);
    f.extend_from_slice(&ssid[..ssid.len().min(32)]);
    f
}

/// Builds an IEEE 802.11 QoS Data header (Protected bit set) + CCMP encrypted payload.
pub fn build_qos_ccmp_frame(
    da: &[u8; 6],
    sa: &[u8; 6],
    bssid: &[u8; 6],
    pn: u64,
    tk: &[u8; 16],
    payload: &[u8],
) -> Vec<u8> {
    let mut hdr = Vec::with_capacity(26);
    hdr.extend_from_slice(&0x4188u16.to_le_bytes()); // Data QoS + ToDS + Protected
    hdr.extend_from_slice(&[0x00, 0x00]);
    hdr.extend_from_slice(bssid);
    hdr.extend_from_slice(sa);
    hdr.extend_from_slice(da);
    hdr.extend_from_slice(&0u16.to_le_bytes());
    hdr.extend_from_slice(&0u16.to_le_bytes()); // QoS Control
    let ccmp = ccmp_encrypt(tk, sa, pn, &hdr, payload);
    let mut frame = hdr;
    let pn_b = pn.to_le_bytes();
    frame.extend_from_slice(&[pn_b[0], pn_b[1], 0x00, 0x20, pn_b[2], pn_b[3], pn_b[4], pn_b[5]]);
    frame.extend_from_slice(&ccmp);
    frame
}

static WLAN0_PROBE_CACHE: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);

/// Returns `/sys/class/net/wlan0/operstate` strictly from real PCIe Wi-Fi hardware presence (cached).
pub fn wlan0_operstate() -> &'static str {
    use core::sync::atomic::Ordering;
    let cached = WLAN0_PROBE_CACHE.load(Ordering::Acquire);
    let present = if cached == 0 {
        let found = ostd::bus::pcie::probe_wifi_controller().is_some();
        WLAN0_PROBE_CACHE.store(if found { 2 } else { 1 }, Ordering::Release);
        found
    } else {
        cached == 2
    };
    if present { "down\n" } else { "no-device\n" }
}

/// Runs a deterministic WPA2-PSK (`PBKDF2-HMAC-SHA1` + `PRF-PTK` + `AES-128-CCMP` + `802.11` framing) self-test.
pub fn run_wpa2_ccmp_selftest() -> Result<alloc::string::String, i64> {
    let pmk = pbkdf2_hmac_sha1(b"FipSOsWPA2Secret", b"FipSOs-Lab", 64);
    let aa = [0x00, 0x11, 0x22, 0x33, 0x44, 0x55];
    let spa = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];
    let anonce = [0xAAu8; 32];
    let snonce = [0x55u8; 32];
    let ptk = derive_ptk(&pmk, &aa, &spa, &anonce, &snonce);
    let mut tk = [0u8; 16];
    tk.copy_from_slice(&ptk[32..48]);

    let auth = build_auth_frame(&aa, &spa, &aa, 1);
    let assoc = build_assoc_req_frame(&aa, &spa, &aa, b"FipSOs-Lab");
    let aad = &assoc[..24];
    let msg = b"IEEE80211-WPA2-AES-CCMP-VERIFIED";
    let mut ct = ccmp_encrypt(&tk, &spa, 1, aad, msg);
    let dec = ccmp_decrypt(&tk, &spa, 1, aad, &ct)?;
    if dec.as_slice() != msg {
        return Err(-5);
    }
    ct[0] ^= 0x01;
    if ccmp_decrypt(&tk, &spa, 1, aad, &ct).is_ok() {
        return Err(-5);
    }
    Ok(format!(
        "WPA2_CCMP_OK auth={}B assoc={}B pmk={:02x}{:02x}.. tk={:02x}{:02x}.. mic=VERIFIED",
        auth.len(),
        assoc.len(),
        pmk[0],
        pmk[1],
        tk[0],
        tk[1]
    ))
}

/// Reads `/mnt/fat0/firmware/<name>` from VFS and stages it via `ostd::bus::firmware_loader`.
pub fn load_wifi_firmware_from_vfs(vfs: &crate::vfs::CapabilityVfs, fw_name: &str) -> Result<u64, i64> {
    if fw_name.is_empty() || fw_name.contains("..") || fw_name.contains('/') {
        return Err(-22);
    }
    let path = format!("/mnt/fat0/firmware/{}", fw_name);
    let bytes = vfs.read_all_by_path(&path)?; // -ENOENT (-2) if file missing
    ostd::bus::firmware_loader::load_wifi_firmware(fw_name, &bytes)
}
