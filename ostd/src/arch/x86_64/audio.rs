//! FipSOs Ring 0 Hardware Audio Synthesizer Driver (`ostd::arch::audio` `/dev/snd/pcm0`).
//!
//! Controls the PC Speaker hardware via Intel 8254 PIT Channel 2 (ports `0x42`/`0x43`)
//! and System Control Port B (`0x61`) to emit non-blocking acoustic signals for
//! network download completion and OS events.

use core::sync::atomic::{AtomicBool, AtomicU16, AtomicU32, Ordering};

const PIT_FREQ_HZ: u32 = 1_193_180;
const PORT_PIT_MODE: u16 = 0x43;
const PORT_PIT_CHAN2: u16 = 0x42;
const PORT_SPEAKER_CTRL: u16 = 0x61;

static BEEPS_COUNT: AtomicU32 = AtomicU32::new(0);
static LAST_FREQ_HZ: AtomicU16 = AtomicU16::new(880);
static LAST_DURATION_MS: AtomicU16 = AtomicU16::new(60);
static SPEAKER_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Point-in-time telemetry snapshot of the PCM/Speaker audio subsystem.
#[derive(Debug, Clone, Copy)]
pub struct AudioTelemetry {
    pub beeps_count: u32,
    pub last_freq_hz: u16,
    pub last_duration_ms: u16,
    pub active: bool,
}

#[inline]
unsafe fn inb(port: u16) -> u8 {
    let value: u8;
    core::arch::asm!("in al, dx", in("dx") port, out("al") value, options(nomem, nostack, preserves_flags));
    value
}

#[inline]
unsafe fn outb(port: u16, value: u8) {
    core::arch::asm!("out dx, al", in("dx") port, in("al") value, options(nomem, nostack, preserves_flags));
}

/// Emits an acoustic tone of frequency `freq_hz` for approximately `duration_ms` milliseconds.
pub fn play_beep(freq_hz: u16, duration_ms: u16) {
    if freq_hz == 0 {
        speaker_off();
        return;
    }

    LAST_FREQ_HZ.store(freq_hz, Ordering::Relaxed);
    LAST_DURATION_MS.store(duration_ms, Ordering::Relaxed);
    BEEPS_COUNT.fetch_add(1, Ordering::Relaxed);
    SPEAKER_ACTIVE.store(true, Ordering::Release);

    let divisor = (PIT_FREQ_HZ / (freq_hz as u32)).clamp(1, 65535) as u16;

    unsafe {
        // Mode 3: Square wave generator on Channel 2, 16-bit binary
        outb(PORT_PIT_MODE, 0xB6);
        outb(PORT_PIT_CHAN2, (divisor & 0xFF) as u8);
        outb(PORT_PIT_CHAN2, ((divisor >> 8) & 0xFF) as u8);

        // Turn on speaker bits (bits 0 and 1 on port 0x61)
        let ctrl = inb(PORT_SPEAKER_CTRL);
        if (ctrl & 0x03) != 0x03 {
            outb(PORT_SPEAKER_CTRL, ctrl | 0x03);
        }

        // Brief calibrated spin for requested duration
        let loops = (duration_ms as u32) * 40_000;
        for _ in 0..loops {
            core::hint::spin_loop();
        }

        // Turn off speaker
        let ctrl_off = inb(PORT_SPEAKER_CTRL);
        outb(PORT_SPEAKER_CTRL, ctrl_off & !0x03);
    }

    SPEAKER_ACTIVE.store(false, Ordering::Release);
}

/// Immediately silences the hardware PC speaker.
pub fn speaker_off() {
    unsafe {
        let ctrl = inb(PORT_SPEAKER_CTRL);
        outb(PORT_SPEAKER_CTRL, ctrl & !0x03);
    }
    SPEAKER_ACTIVE.store(false, Ordering::Release);
}

/// Returns a telemetry snapshot of the audio device (`/dev/snd/pcm0`).
pub fn snapshot() -> AudioTelemetry {
    AudioTelemetry {
        beeps_count: BEEPS_COUNT.load(Ordering::Relaxed),
        last_freq_hz: LAST_FREQ_HZ.load(Ordering::Relaxed),
        last_duration_ms: LAST_DURATION_MS.load(Ordering::Relaxed),
        active: SPEAKER_ACTIVE.load(Ordering::Acquire),
    }
}
