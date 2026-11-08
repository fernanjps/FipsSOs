//! CMOS Real-Time Clock (RTC) and ACPI Hardware Power Management (Shutdown / Reboot).

const CMOS_ADDR: u16 = 0x70;
const CMOS_DATA: u16 = 0x71;

const REG_SEC: u8 = 0x00;
const REG_MIN: u8 = 0x02;
const REG_HOUR: u8 = 0x04;
const REG_DAY: u8 = 0x07;
const REG_MONTH: u8 = 0x08;
const REG_YEAR: u8 = 0x09;
const REG_STATUS_A: u8 = 0x0A;
const REG_STATUS_B: u8 = 0x0B;

/// Real-Time Clock (RTC) timestamp representing wall-clock date and time.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RtcDateTime {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

#[inline]
unsafe fn cmos_read(reg: u8) -> u8 {
    // Bit 7 disables NMI; we keep NMI disabled during the brief read
    core::arch::asm!(
        "out dx, al",
        in("dx") CMOS_ADDR,
        in("al") reg | 0x80,
        options(nomem, nostack, preserves_flags)
    );
    let val: u8;
    core::arch::asm!(
        "in al, dx",
        in("dx") CMOS_DATA,
        out("al") val,
        options(nomem, nostack, preserves_flags)
    );
    val
}

#[inline]
fn bcd_to_bin(val: u8) -> u8 {
    (val & 0x0F) + ((val / 16) * 10)
}

/// Reads the current date and time from the CMOS RTC hardware registers (`0x70` / `0x71`).
pub fn read_rtc() -> RtcDateTime {
    // Wait until Update In Progress (UIP, bit 7 of Status Register A) clears
    for _ in 0..1000 {
        // SAFETY: Reading standard CMOS Status Register A.
        let status_a = unsafe { cmos_read(REG_STATUS_A) };
        if (status_a & 0x80) == 0 {
            break;
        }
    }

    // SAFETY: Reading standard CMOS RTC time and date registers.
    let (mut sec, mut min, mut hour, mut day, mut month, mut year, status_b) = unsafe {
        (
            cmos_read(REG_SEC),
            cmos_read(REG_MIN),
            cmos_read(REG_HOUR),
            cmos_read(REG_DAY),
            cmos_read(REG_MONTH),
            cmos_read(REG_YEAR),
            cmos_read(REG_STATUS_B),
        )
    };

    let is_bcd = (status_b & 0x04) == 0;
    if is_bcd {
        sec = bcd_to_bin(sec);
        min = bcd_to_bin(min);
        hour = ((hour & 0x7F) & 0x0F) + (((hour & 0x7F) / 16) * 10) | (hour & 0x80);
        day = bcd_to_bin(day);
        month = bcd_to_bin(month);
        year = bcd_to_bin(year);
    }

    // Convert 12-hour format to 24-hour if bit 1 of Status Register B is 0
    let is_24h = (status_b & 0x02) != 0;
    if !is_24h && (hour & 0x80) != 0 {
        hour = ((hour & 0x7F) + 12) % 24;
    }

    // Assume 2000s for years < 70, 1900s otherwise
    let full_year = if year < 70 {
        2000u16 + (year as u16)
    } else {
        1900u16 + (year as u16)
    };

    RtcDateTime {
        year: full_year,
        month: month.clamp(1, 12),
        day: day.clamp(1, 31),
        hour: hour.clamp(0, 23),
        minute: min.clamp(0, 59),
        second: sec.clamp(0, 59),
    }
}

/// Converts an `RtcDateTime` into a standard POSIX UNIX timestamp (seconds since 1970-01-01 00:00:00 UTC).
pub fn rtc_to_unix_timestamp(dt: &RtcDateTime) -> i64 {
    let y = dt.year as i64;
    let m = dt.month as i64;
    let d = dt.day as i64;

    // Cumulative days before each month in a non-leap year
    const DAYS_BEFORE_MONTH: [i64; 12] = [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];

    fn is_leap_year(year: i64) -> bool {
        (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
    }

    let mut days = (y - 1970) * 365;
    // Add leap days between 1970 and y
    for yr in 1970..y {
        if is_leap_year(yr) {
            days += 1;
        }
    }

    let m_idx = (m - 1).clamp(0, 11) as usize;
    days += DAYS_BEFORE_MONTH[m_idx];
    if m > 2 && is_leap_year(y) {
        days += 1;
    }
    days += d - 1;

    days * 86400 + (dt.hour as i64) * 3600 + (dt.minute as i64) * 60 + (dt.second as i64)
}

/// Shuts down the system using ACPI PM1a_CNT S5 sleep state (QEMU `q35` `0x604` and `0xB004`).
pub fn system_shutdown() -> ! {
    crate::early_println!("[ACPI::Power] Apagando el equipo (ACPI S5 Power Off)...");
    crate::fb_println!("[ACPI] Apagando el equipo (ACPI S5)...");

    // SAFETY: Writing standard ACPI PM1a_CNT S5 sleep bits in Ring 0.
    unsafe {
        // QEMU q35 ACPI PM1a_CNT port: 0x604
        core::arch::asm!(
            "out dx, ax",
            in("dx") 0x604u16,
            in("ax") 0x2000u16,
            options(nomem, nostack)
        );
        // QEMU legacy PIIX4 ACPI PM1a_CNT port: 0xB004
        core::arch::asm!(
            "out dx, ax",
            in("dx") 0xB004u16,
            in("ax") 0x2000u16,
            options(nomem, nostack)
        );
        // QEMU alternative sleep enable
        core::arch::asm!(
            "out dx, ax",
            in("dx") 0x604u16,
            in("ax") 0x3400u16,
            options(nomem, nostack)
        );
    }

    crate::halt_loop()
}

/// Restarts the system via the Reset Control Register (`0xCF9 = 0x06`) and PS/2 keyboard controller (`0x64 = 0xFE`).
pub fn system_reboot() -> ! {
    crate::early_println!("[ACPI::Power] Reiniciando el equipo (Hardware Reset 0xCF9 / 0x64)...");
    crate::fb_println!("[ACPI] Reiniciando el equipo (Hardware Reset)...");

    // SAFETY: Writing standard x86 Reset Control Register (0xCF9) and PS/2 pulse (0x64).
    unsafe {
        // Reset Control Register: write 0x02, then 0x06
        core::arch::asm!(
            "out dx, al",
            in("dx") 0xCF9u16,
            in("al") 0x02u8,
            options(nomem, nostack)
        );
        core::arch::asm!(
            "out dx, al",
            in("dx") 0xCF9u16,
            in("al") 0x06u8,
            options(nomem, nostack)
        );
        // PS/2 Controller pulse reset line
        core::arch::asm!(
            "out dx, al",
            in("dx") 0x64u16,
            in("al") 0xFEu8,
            options(nomem, nostack)
        );

        // Fallback: Triple fault via null IDT
        let null_idt = [0u8; 10];
        core::arch::asm!(
            "lidt [{}]",
            "int 3",
            in(reg) null_idt.as_ptr(),
            options(noreturn)
        );
    }
}
