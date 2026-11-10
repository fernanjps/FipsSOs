//! PS/2 Auxiliary Mouse Driver (`IRQ12`, Vector `44` / `0x2C`) with 3-Byte Packet State Machine,
//! Screen-Clamped Coordinates (`1280x800`), and Synthetic Pointer Injection (`ostd::arch::x86_64::mouse`).

use crate::sync::SpinLock;

/// PS/2 8042 Controller I/O ports.
pub const PS2_DATA_PORT: u16 = 0x60;
pub const PS2_STATUS_CMD_PORT: u16 = 0x64;

/// PS/2 status register flags (`0x64`).
const STATUS_OUTPUT_FULL: u8 = 1 << 0; // Bit 0: Output buffer has data ready on 0x60
const STATUS_INPUT_BUSY: u8 = 1 << 1; // Bit 1: Input buffer busy (wait until 0 before write)
const STATUS_AUX_DATA: u8 = 1 << 5; // Bit 5: Byte in 0x60 originated from PS/2 Mouse (AUX)

/// Default framebuffer coordinate bounds (`1280x800`) and center position (`640, 400`).
pub const DEFAULT_SCREEN_WIDTH: i32 = 1280;
pub const DEFAULT_SCREEN_HEIGHT: i32 = 800;
pub const DEFAULT_MOUSE_X: i32 = DEFAULT_SCREEN_WIDTH / 2;
pub const DEFAULT_MOUSE_Y: i32 = DEFAULT_SCREEN_HEIGHT / 2;

/// Mouse button bitmasks matching PS/2 packet byte 0 (`bits 0..2`).
pub const MOUSE_BTN_LEFT: u8 = 1 << 0;
pub const MOUSE_BTN_RIGHT: u8 = 1 << 1;
pub const MOUSE_BTN_MIDDLE: u8 = 1 << 2;

/// Snapshot of the current PS/2 pointer coordinates, button flags, and packet telemetry.
#[derive(Debug, Clone, Copy)]
pub struct MouseSnapshot {
    pub x: u16,
    pub y: u16,
    pub buttons: u8,
    pub left: bool,
    pub right: bool,
    pub middle: bool,
    pub event_seq: u64,
    pub click_count: u64,
}

impl MouseSnapshot {
    /// Packs the pointer state into a single 64-bit word for `MOUSE_IOCTL_POLL_STATE`:
    /// - Bits `0..15`  : `x` (`0..1279`)
    /// - Bits `16..31` : `y` (`0..799`)
    /// - Bits `32..39` : `buttons` (`bit 0 = left`, `bit 1 = right`, `bit 2 = middle`)
    /// - Bits `40..63` : `event_seq` (24-bit monotonic packet/event counter)
    #[inline]
    pub const fn pack_u64(&self) -> u64 {
        (self.x as u64)
            | ((self.y as u64) << 16)
            | ((self.buttons as u64) << 32)
            | ((self.event_seq & 0x00FF_FFFF) << 40)
    }
}

struct MouseState {
    packet: [u8; 3],
    cycle: u8,
    x: i32,
    y: i32,
    max_x: i32,
    max_y: i32,
    buttons: u8,
    event_seq: u64,
    click_count: u64,
}

impl MouseState {
    const fn new() -> Self {
        Self {
            packet: [0; 3],
            cycle: 0,
            x: DEFAULT_MOUSE_X,
            y: DEFAULT_MOUSE_Y,
            max_x: DEFAULT_SCREEN_WIDTH - 1,
            max_y: DEFAULT_SCREEN_HEIGHT - 1,
            buttons: 0,
            event_seq: 0,
            click_count: 0,
        }
    }

    fn set_bounds(&mut self, width: usize, height: usize) {
        if width >= 320 && height >= 200 {
            self.max_x = (width as i32) - 1;
            self.max_y = (height as i32) - 1;
            self.x = (width as i32) / 2;
            self.y = (height as i32) / 2;
        }
    }

    fn snapshot(&self) -> MouseSnapshot {
        let buttons = self.buttons;
        MouseSnapshot {
            x: self.x.clamp(0, self.max_x) as u16,
            y: self.y.clamp(0, self.max_y) as u16,
            buttons,
            left: (buttons & MOUSE_BTN_LEFT) != 0,
            right: (buttons & MOUSE_BTN_RIGHT) != 0,
            middle: (buttons & MOUSE_BTN_MIDDLE) != 0,
            event_seq: self.event_seq,
            click_count: self.click_count,
        }
    }

    /// Processes a single byte from the 3-byte PS/2 mouse packet stream `[flags, dx, dy]`.
    /// Returns `Some((x, y, buttons))` whenever a complete 3-byte packet commits.
    fn feed_byte(&mut self, byte: u8) -> Option<(usize, usize, u8)> {
        match self.cycle {
            0 => {
                // Bit 3 (0x08) must be 1 in a valid PS/2 byte-0 header; ignore X/Y overflow packets (0xC0)
                if (byte & 0x08) != 0 && (byte & 0xC0) == 0 {
                    self.packet[0] = byte;
                    self.cycle = 1;
                }
                None
            }
            1 => {
                self.packet[1] = byte;
                self.cycle = 2;
                None
            }
            _ => {
                self.packet[2] = byte;
                self.cycle = 0;

                let flags = self.packet[0];
                let raw_dx = self.packet[1] as i32;
                let raw_dy = self.packet[2] as i32;

                // Sign-extend 9-bit two's-complement deltas using sign bits 4 (X) and 5 (Y)
                let dx = raw_dx - if (flags & 0x10) != 0 { 256 } else { 0 };
                let dy_ps2 = raw_dy - if (flags & 0x20) != 0 { 256 } else { 0 };

                // Invert PS/2 Y axis (+Y up -> screen +Y down) and clamp to framebuffer bounds
                self.x = (self.x + dx).clamp(0, self.max_x);
                self.y = (self.y - dy_ps2).clamp(0, self.max_y);

                let new_buttons = flags & 0x07;
                if (new_buttons & !self.buttons) != 0 {
                    self.click_count = self.click_count.wrapping_add(1);
                }
                self.buttons = new_buttons;
                self.event_seq = self.event_seq.wrapping_add(1);

                Some((self.x as usize, self.y as usize, self.buttons))
            }
        }
    }

    fn apply_synthetic_step(&mut self, dx: i16, dy: i16, buttons: u8) -> (usize, usize, u8, MouseSnapshot) {
        self.x = (self.x + (dx as i32)).clamp(0, self.max_x);
        self.y = (self.y + (dy as i32)).clamp(0, self.max_y);

        let clean_buttons = buttons & 0x07;
        if clean_buttons != 0 {
            self.click_count = self.click_count.wrapping_add(1);
        }
        self.buttons = clean_buttons;
        self.event_seq = self.event_seq.wrapping_add(1);

        let snap = self.snapshot();
        (self.x as usize, self.y as usize, self.buttons, snap)
    }
}

static MOUSE: SpinLock<MouseState> = SpinLock::new(MouseState::new());

/// Initializes the PS/2 Auxiliary Mouse device on controller ports `0x64`/`0x60`:
/// 1. Sends `0xA8` to enable the auxiliary mouse port.
/// 2. Reads Compaq Status Byte (`0x20`), sets bit 1 (`IRQ12` enable) and clears bit 5 (aux clock), writes back via `0x60`.
/// 3. Sends `0xD4` + `0xF6` (Set Defaults) and `0xD4` + `0xF4` (Enable Data Reporting).
pub fn init() {
    let (fb_w, fb_h) = crate::fb_console::dimensions();
    {
        let mut guard = MOUSE.lock();
        guard.set_bounds(fb_w, fb_h);
    }

    let compaq_after: u8;
    let ack_defaults: u8;
    let ack_enable: u8;

    // SAFETY: Programming the 8042 PS/2 auxiliary controller in Ring 0 during single-threaded boot.
    unsafe {
        drain_output_buffer();

        // 1. Enable PS/2 Auxiliary Device port (0xA8)
        wait_ps2_write();
        outb(PS2_STATUS_CMD_PORT, 0xA8);

        // 2. Read & update Compaq Status / Controller Configuration Byte (0x20 -> 0x60)
        wait_ps2_write();
        outb(PS2_STATUS_CMD_PORT, 0x20);
        let mut config = if wait_ps2_read() {
            inb(PS2_DATA_PORT)
        } else {
            0x45
        };
        config |= 1 << 1; // Bit 1 = Enable IRQ12
        config &= !(1 << 5); // Bit 5 = 0 (Enable Auxiliary Mouse Clock)
        compaq_after = config;

        wait_ps2_write();
        outb(PS2_STATUS_CMD_PORT, 0x60);
        wait_ps2_write();
        outb(PS2_DATA_PORT, config);

        // 3. Set Default Mouse Settings (0xD4 -> 0xF6) and Enable Packet Streaming (0xD4 -> 0xF4)
        ack_defaults = send_mouse_command(0xF6);
        ack_enable = send_mouse_command(0xF4);

        drain_output_buffer();
    }

    let snap = MOUSE.lock().snapshot();

    crate::early_println!(
        "[ostd::arch::mouse] Raton PS/2 IRQ12 (vec=0x2c) activo | Compaq={:#04x}, ACK={:#04x}/{:#04x} | Pos=({}, {})",
        compaq_after,
        ack_defaults,
        ack_enable,
        snap.x,
        snap.y
    );
    crate::fb_println!(
        "[ostd::arch::mouse] Raton PS/2 IRQ12 (vec=0x2c) activo | ACK={:#04x} | Cursor=({}, {})",
        ack_enable,
        snap.x,
        snap.y
    );
}

/// Feeds a raw auxiliary PS/2 byte into the 3-byte mouse state machine and updates coordinates.
/// In text TTY console mode, does not repaint pixel overlays on VRAM to prevent bus contention with typing.
pub(crate) fn process_packet_byte(byte: u8) {
    let committed = if let Some(mut guard) = MOUSE.try_lock() {
        guard.feed_byte(byte)
    } else {
        None
    };

    if let Some((x, y, buttons)) = committed {
        if crate::fb_console::is_desktop_mode() {
            crate::fb_console::draw_mouse_cursor_overlay(x, y, buttons);
        }
    }
}

/// Invoked from `__rust_mouse_irq_handler` (`IRQ12`, Vector `44` / `0x2C`).
pub(crate) fn on_irq12() {
    // SAFETY: Reading 8042 status (`0x64`) and auxiliary data (`0x60`) inside `IRQ12` ISR.
    unsafe {
        for _ in 0..6 {
            let status = inb(PS2_STATUS_CMD_PORT);
            if (status & (STATUS_OUTPUT_FULL | STATUS_AUX_DATA))
                != (STATUS_OUTPUT_FULL | STATUS_AUX_DATA)
            {
                break;
            }
            let data = inb(PS2_DATA_PORT);
            process_packet_byte(data);
        }
    }
}

/// Drains any pending auxiliary mouse bytes waiting in the 8042 controller and returns the latest `MouseSnapshot`.
pub fn poll_state() -> MouseSnapshot {
    // SAFETY: Checking PS/2 controller status (`0x64`) for pending auxiliary bytes (`bit 5 | bit 0`).
    unsafe {
        for _ in 0..6 {
            let status = inb(PS2_STATUS_CMD_PORT);
            if (status & (STATUS_OUTPUT_FULL | STATUS_AUX_DATA))
                == (STATUS_OUTPUT_FULL | STATUS_AUX_DATA)
            {
                let data = inb(PS2_DATA_PORT);
                process_packet_byte(data);
            } else {
                break;
            }
        }
    }
    MOUSE.lock().snapshot()
}

/// Injects a synthetic relative pointer movement `(dx, dy)` and button mask (`left=1, right=2, middle=4`)
/// so pointer events and clicks can also be tested from keyboard/console without host mouse grab.
pub fn inject_synthetic_mouse_step(dx: i16, dy: i16, buttons: u8) -> MouseSnapshot {
    let (x, y, btn, snap) = MOUSE.lock().apply_synthetic_step(dx, dy, buttons);
    crate::fb_console::draw_mouse_cursor_overlay(x, y, btn);
    snap
}

#[inline]
unsafe fn send_mouse_command(cmd: u8) -> u8 {
    // SAFETY: Caller guarantees Ring 0 PS/2 initialization context.
    unsafe {
        wait_ps2_write();
        outb(PS2_STATUS_CMD_PORT, 0xD4);
        wait_ps2_write();
        outb(PS2_DATA_PORT, cmd);
        if wait_ps2_read() {
            inb(PS2_DATA_PORT)
        } else {
            0x00
        }
    }
}

#[inline]
unsafe fn wait_ps2_write() {
    // SAFETY: Polling read-only PS/2 status port 0x64.
    unsafe {
        for _ in 0..100_000 {
            if (inb(PS2_STATUS_CMD_PORT) & STATUS_INPUT_BUSY) == 0 {
                return;
            }
            core::hint::spin_loop();
        }
    }
}

#[inline]
unsafe fn wait_ps2_read() -> bool {
    // SAFETY: Polling read-only PS/2 status port 0x64.
    unsafe {
        for _ in 0..100_000 {
            if (inb(PS2_STATUS_CMD_PORT) & STATUS_OUTPUT_FULL) != 0 {
                return true;
            }
            core::hint::spin_loop();
        }
    }
    false
}

#[inline]
unsafe fn drain_output_buffer() {
    // SAFETY: Draining pending bytes from PS/2 data port 0x60.
    unsafe {
        for _ in 0..32 {
            if (inb(PS2_STATUS_CMD_PORT) & STATUS_OUTPUT_FULL) != 0 {
                let _ = inb(PS2_DATA_PORT);
            } else {
                break;
            }
        }
    }
}

#[inline]
unsafe fn inb(port: u16) -> u8 {
    let value: u8;
    // SAFETY: Caller guarantees valid x86 I/O port.
    unsafe {
        core::arch::asm!(
            "in al, dx",
            out("al") value,
            in("dx") port,
            options(nomem, nostack, preserves_flags)
        );
    }
    value
}

#[inline]
unsafe fn outb(port: u16, value: u8) {
    // SAFETY: Caller guarantees valid x86 I/O port.
    unsafe {
        core::arch::asm!(
            "out dx, al",
            in("dx") port,
            in("al") value,
            options(nomem, nostack, preserves_flags)
        );
    }
}
