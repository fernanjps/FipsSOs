//! ACPI Embedded Controller (EC) Battery & Real MSR Turbo/CPB Driver (`x86_64`).
//!
//! Zero simulated fallbacks: if no ACPI EC battery responds on `0x62`/`0x66`,
//! `BatterySnapshot` reports `present: false`, `capacity_pct: None`, and `status: "NotPresent"`.
//! Turbo Boost / Core Performance Boost is controlled via real AMD/Intel MSRs when
//! CPUID confirms bare-metal hardware support.

use core::sync::atomic::{AtomicBool, AtomicI8, AtomicU16, AtomicU64, AtomicU8, Ordering};

const EC_DATA_PORT: u16 = 0x62;
const EC_SC_PORT: u16 = 0x66;
const EC_OBF: u8 = 0x01;
const EC_IBF: u8 = 0x02;
const EC_CMD_READ: u8 = 0x80;
const EC_REG_AC_STATUS: u8 = 0x10;
const EC_REG_BAT_CAP: u8 = 0x20;
const EC_POLL_LIMIT: usize = 256;

/// Active CPU/platform power management profile.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerProfile {
    Performance = 0,
    Balanced = 1,
    Powersave = 2,
}

impl PowerProfile {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Performance => "performance",
            Self::Balanced => "balanced",
            Self::Powersave => "powersave",
        }
    }

    pub fn from_str(raw: &str) -> Option<Self> {
        let t = raw.trim();
        if t.eq_ignore_ascii_case("performance") { Some(Self::Performance) }
        else if t.eq_ignore_ascii_case("balanced") { Some(Self::Balanced) }
        else if t.eq_ignore_ascii_case("powersave") { Some(Self::Powersave) }
        else { None }
    }

    pub const fn idle_pause_iters(self) -> usize {
        match self {
            Self::Performance => 16,
            Self::Balanced => 128,
            Self::Powersave => 256,
        }
    }

    pub const fn idle_hlt_cycles(self) -> usize {
        match self {
            Self::Performance => 1,
            Self::Balanced => 1,
            Self::Powersave => 2,
        }
    }
}

/// Real hardware battery and Turbo Boost telemetry snapshot (no simulated values).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BatterySnapshot {
    pub present: bool,
    pub ac_online: Option<bool>,
    pub capacity_pct: Option<u8>,
    pub status: &'static str,
    pub profile: PowerProfile,
    pub turbo_hw_supported: bool,
    pub turbo_on: bool,
}

static ACTIVE_PROFILE: AtomicU8 = AtomicU8::new(PowerProfile::Balanced as u8);
// EC hardware presence tri-state: -1 = unprobed, 0 = absent/VM (never touch 0x62/0x66), 1 = detected.
static EC_PRESENT: AtomicI8 = AtomicI8::new(-1);
static LAST_EC_TICK: AtomicU64 = AtomicU64::new(0);
// Packed atomic snapshot: bit 8 = ac_online, bits 0..=7 = battery capacity [0..=100].
static CACHED_EC_STATE: AtomicU16 = AtomicU16::new(0);
static TURBO_HW_SUPPORTED: AtomicBool = AtomicBool::new(false);
static TURBO_ON: AtomicBool = AtomicBool::new(false);
static EC_LOCK: crate::sync::SpinLock<()> = crate::sync::SpinLock::new(());

#[inline]
const fn pack_ec_state(ac_online: bool, cap: u8) -> u16 {
    ((ac_online as u16) << 8) | (cap as u16)
}

#[inline]
const fn unpack_ec_state(raw: u16) -> (bool, u8) {
    (((raw >> 8) & 1) != 0, (raw & 0xFF) as u8)
}

#[inline]
unsafe fn inb(port: u16) -> u8 {
    let val: u8;
    core::arch::asm!("in al, dx", in("dx") port, out("al") val, options(nomem, nostack, preserves_flags));
    val
}

#[inline]
unsafe fn outb(port: u16, val: u8) {
    core::arch::asm!("out dx, al", in("dx") port, in("al") val, options(nomem, nostack, preserves_flags));
}

fn ec_wait_flag(mask: u8, want_set: bool) -> bool {
    let deadline = crate::time::tsc_deadline_ms(1);
    for _ in 0..EC_POLL_LIMIT {
        if crate::time::tsc_expired(deadline) {
            return false;
        }
        // SAFETY: Reading standard ACPI EC status port 0x66.
        let st = unsafe { inb(EC_SC_PORT) };
        if st == 0xFF {
            return false;
        }
        if ((st & mask) != 0) == want_set {
            return true;
        }
        core::arch::x86_64::_mm_pause();
    }
    false
}

fn ec_read_reg(reg: u8) -> Option<u8> {
    if EC_PRESENT.load(Ordering::Acquire) == 0 {
        return None;
    }
    let _guard = EC_LOCK.lock();
    // SAFETY: Checking EC status port 0x66 before issuing EC read sequence.
    let initial = unsafe { inb(EC_SC_PORT) };
    if initial == 0xFF || initial == 0x00 {
        return None;
    }
    if (initial & EC_OBF) != 0 {
        // SAFETY: Drain stale OBF byte from 0x62 before issuing command.
        let _ = unsafe { inb(EC_DATA_PORT) };
    }
    if !ec_wait_flag(EC_IBF, false) {
        return None;
    }
    // SAFETY: Issuing standard ACPI EC_READ (0x80) to 0x66.
    unsafe { outb(EC_SC_PORT, EC_CMD_READ) };
    if !ec_wait_flag(EC_IBF, false) {
        return None;
    }
    // SAFETY: Writing EC register offset to 0x62.
    unsafe { outb(EC_DATA_PORT, reg) };
    if !ec_wait_flag(EC_OBF, true) {
        return None;
    }
    // SAFETY: Reading EC register value from 0x62.
    Some(unsafe { inb(EC_DATA_PORT) })
}

fn detect_turbo_msr() -> Option<(u32, u64)> {
    let is_hypervisor = (core::arch::x86_64::__cpuid(1).ecx & (1 << 31)) != 0;
    if is_hypervisor {
        return None;
    }
    let topo = super::topology::detect();
    if topo.vendor() == "AuthenticAMD"
        && core::arch::x86_64::__cpuid(0x8000_0000).eax >= 0x8000_0007
        && (core::arch::x86_64::__cpuid(0x8000_0007).edx & (1 << 9)) != 0
    {
        Some((0xC001_0015u32, 1u64 << 25))
    } else if topo.vendor() == "GenuineIntel"
        && core::arch::x86_64::__cpuid(0).eax >= 6
        && (core::arch::x86_64::__cpuid(6).eax & (1 << 1)) != 0
    {
        Some((0x1A0u32, 1u64 << 38))
    } else {
        None
    }
}

fn sync_turbo_msr(write_enable: Option<bool>) {
    if let Some((msr, bit)) = detect_turbo_msr() {
        if let Some(cur) = super::msr::rdmsr_safe(msr) {
            TURBO_HW_SUPPORTED.store(true, Ordering::Release);
            let final_val = if let Some(enable) = write_enable {
                let next = if enable { cur & !bit } else { cur | bit };
                if next != cur && super::msr::wrmsr_safe(msr, next) {
                    next
                } else {
                    cur
                }
            } else {
                cur
            };
            TURBO_ON.store((final_val & bit) == 0, Ordering::Release);
            return;
        }
    }
    TURBO_HW_SUPPORTED.store(false, Ordering::Release);
    TURBO_ON.store(false, Ordering::Release);
}

/// Probes real ACPI EC hardware and CPU Turbo MSR state (no fake fallbacks).
pub fn init() {
    let is_hypervisor = (core::arch::x86_64::__cpuid(1).ecx & (1 << 31)) != 0;
    let mut ec_ok = false;
    if !is_hypervisor {
        // SAFETY: Bare-metal check of ACPI EC status port 0x66 once at boot.
        let status = unsafe { inb(EC_SC_PORT) };
        if status != 0xFF && status != 0x00 {
            if let (Some(ac_raw), Some(cap_raw)) =
                (ec_read_reg(EC_REG_AC_STATUS), ec_read_reg(EC_REG_BAT_CAP))
            {
                if cap_raw <= 100 {
                    EC_PRESENT.store(1, Ordering::Release);
                    CACHED_EC_STATE.store(pack_ec_state((ac_raw & 0x01) != 0, cap_raw), Ordering::Release);
                    LAST_EC_TICK.store(crate::arch::x86_64::idt::timer_ticks(), Ordering::Release);
                    ec_ok = true;
                }
            }
        }
    }
    if !ec_ok {
        EC_PRESENT.store(0, Ordering::Release);
    }
    sync_turbo_msr(None);
    crate::early_println!(
        "[ostd::power] EC_BAT={} | TurboMSR={}",
        if ec_ok { "DETECTED" } else { "NotPresent" },
        boost_str()
    );
}

pub fn profile() -> PowerProfile {
    match ACTIVE_PROFILE.load(Ordering::Acquire) {
        0 => PowerProfile::Performance,
        2 => PowerProfile::Powersave,
        _ => PowerProfile::Balanced,
    }
}

pub fn set_profile(name: &str) -> Result<PowerProfile, ()> {
    let parsed = PowerProfile::from_str(name).ok_or(())?;
    ACTIVE_PROFILE.store(parsed as u8, Ordering::Release);
    sync_turbo_msr(Some(!matches!(parsed, PowerProfile::Powersave)));
    Ok(parsed)
}

#[inline]
pub fn idle_pause_iters() -> usize {
    profile().idle_pause_iters()
}

#[inline]
pub fn idle_hlt_cycles() -> usize {
    profile().idle_hlt_cycles()
}

#[inline]
pub fn boost_str() -> &'static str {
    if !TURBO_HW_SUPPORTED.load(Ordering::Acquire) {
        "NO_HW_MSR"
    } else if TURBO_ON.load(Ordering::Acquire) {
        "TURBO_ON"
    } else {
        "BASE_ECO"
    }
}

/// Captures real hardware battery and Turbo Boost state (`NotPresent` / `None` if no EC).
pub fn snapshot() -> BatterySnapshot {
    if EC_PRESENT.load(Ordering::Acquire) == 1 {
        let now = crate::arch::x86_64::idt::timer_ticks();
        let last = LAST_EC_TICK.load(Ordering::Acquire);
        if now.saturating_sub(last) >= 200
            && LAST_EC_TICK
                .compare_exchange(last, now, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
        {
            if let (Some(ac_raw), Some(cap_raw)) =
                (ec_read_reg(EC_REG_AC_STATUS), ec_read_reg(EC_REG_BAT_CAP))
            {
                if cap_raw <= 100 {
                    CACHED_EC_STATE.store(
                        pack_ec_state((ac_raw & 0x01) != 0, cap_raw),
                        Ordering::Release,
                    );
                }
            }
        }
        let (ac, cap) = unpack_ec_state(CACHED_EC_STATE.load(Ordering::Acquire));
        let status = if ac {
            if cap >= 100 { "Full" } else { "Charging" }
        } else {
            "Discharging"
        };
        return BatterySnapshot {
            present: true,
            ac_online: Some(ac),
            capacity_pct: Some(cap),
            status,
            profile: profile(),
            turbo_hw_supported: TURBO_HW_SUPPORTED.load(Ordering::Acquire),
            turbo_on: TURBO_ON.load(Ordering::Acquire),
        };
    }

    BatterySnapshot {
        present: false,
        ac_online: None,
        capacity_pct: None,
        status: "NotPresent",
        profile: profile(),
        turbo_hw_supported: TURBO_HW_SUPPORTED.load(Ordering::Acquire),
        turbo_on: TURBO_ON.load(Ordering::Acquire),
    }
}
