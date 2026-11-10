use core::task::Waker;

use crate::sync::SpinLock;

/// PS/2 Controller I/O ports.
pub const PS2_DATA_PORT: u16 = 0x60;
pub const PS2_STATUS_PORT: u16 = 0x64;

const KEY_RING_CAPACITY: usize = 256;

/// Scancode Set 1 unshifted lookup table for make codes `0x00..0x5D` (`0x5B`/`0x5C` = Windows Super Key -> `0x11`).
static SCANCODE_SET1_NORMAL: [u8; 93] = [
    0,    0x1B, b'1', b'2', b'3', b'4', b'5', b'6', // 0x00..0x07
    b'7', b'8', b'9', b'0', b'-', b'=', 0x08, b' ', // 0x08..0x0F (0x0E = Backspace, 0x0F = Tab->Space)
    b'q', b'w', b'e', b'r', b't', b'y', b'u', b'i', // 0x10..0x17
    b'o', b'p', b'[', b']', b'\n', 0,   b'a', b's', // 0x18..0x1F (0x1C = Enter, 0x1D = LCtrl)
    b'd', b'f', b'g', b'h', b'j', b'k', b'l', b';', // 0x20..0x27
    b'\'', b'`', 0,   b'\\', b'z', b'x', b'c', b'v', // 0x28..0x2F (0x2A = LShift)
    b'b', b'n', b'm', b',', b'.', b'/', 0,    b'*', // 0x30..0x37 (0x36 = RShift)
    0,    b' ', 0,    0,    0,    0,    0,    0,    // 0x38..0x3F (0x39 = Space, 0x3A = CapsLock)
    0,    0,    0,    0,    0,    0,    0,    b'7', // 0x40..0x47
    b'8', b'9', b'-', b'4', b'5', b'6', b'+', b'1', // 0x48..0x4F
    b'2', b'3', b'0', b'.', 0,    0,    0,    0,    // 0x50..0x57
    0,    0,    0,    0x11, 0x11,                   // 0x58..0x5C (0x5B = LWin, 0x5C = RWin)
];

/// Scancode Set 1 shifted lookup table for make codes `0x00..0x5D`.
static SCANCODE_SET1_SHIFTED: [u8; 93] = [
    0,    0x1B, b'!', b'@', b'#', b'$', b'%', b'^', // 0x00..0x07
    b'&', b'*', b'(', b')', b'_', b'+', 0x08, b' ', // 0x08..0x0F
    b'Q', b'W', b'E', b'R', b'T', b'Y', b'U', b'I', // 0x10..0x17
    b'O', b'P', b'{', b'}', b'\n', 0,   b'A', b'S', // 0x18..0x1F
    b'D', b'F', b'G', b'H', b'J', b'K', b'L', b':', // 0x20..0x27
    b'"', b'~', 0,    b'|', b'Z', b'X', b'C', b'V', // 0x28..0x2F
    b'B', b'N', b'M', b'<', b'>', b'?', 0,    b'*', // 0x30..0x37
    0,    b' ', 0,    0,    0,    0,    0,    0,    // 0x38..0x3F
    0,    0,    0,    0,    0,    0,    0,    b'7', // 0x40..0x47
    b'8', b'9', b'-', b'4', b'5', b'6', b'+', b'1', // 0x48..0x4F
    b'2', b'3', b'0', b'.', 0,    0,    0,    0,    // 0x50..0x57
    0,    0,    0,    0x11, 0x11,                   // 0x58..0x5C
];

struct KeyboardState {
    buf: [u8; KEY_RING_CAPACITY],
    head: usize,
    tail: usize,
    len: usize,
    left_shift: bool,
    right_shift: bool,
    left_alt: bool,
    right_alt: bool,
    caps_lock: bool,
    extended: bool,
    waker: Option<Waker>,
}

impl KeyboardState {
    const fn new() -> Self {
        Self {
            buf: [0; KEY_RING_CAPACITY],
            head: 0,
            tail: 0,
            len: 0,
            left_shift: false,
            right_shift: false,
            left_alt: false,
            right_alt: false,
            caps_lock: false,
            extended: false,
            waker: None,
        }
    }

    fn push_byte(&mut self, byte: u8) -> Option<Waker> {
        if self.len < KEY_RING_CAPACITY {
            self.buf[self.tail] = byte;
            self.tail = (self.tail + 1) % KEY_RING_CAPACITY;
            self.len += 1;
        }
        self.waker.take()
    }

    fn pop_byte(&mut self) -> Option<u8> {
        if self.len == 0 {
            return None;
        }
        let byte = self.buf[self.head];
        self.head = (self.head + 1) % KEY_RING_CAPACITY;
        self.len -= 1;
        Some(byte)
    }

    fn handle_scancode(&mut self, scancode: u8) -> Option<Waker> {
        if scancode == 0xE0 {
            self.extended = true;
            return None;
        }

        if self.extended {
            self.extended = false;
            // Handle break codes for extended keys
            if (scancode & 0x80) != 0 {
                let make = scancode & 0x7F;
                if make == 0x38 {
                    self.right_alt = false;
                }
                return None;
            }
            if scancode == 0x38 {
                self.right_alt = true;
                return None;
            }
            return match scancode {
                // Page Up (RePag) -> Scroll console backward 10 lines
                0x49 => {
                    crate::fb_console::scroll_history_backward(10);
                    None
                }
                // Page Down (AvPag) -> Scroll console forward 10 lines
                0x51 => {
                    crate::fb_console::scroll_history_forward(10);
                    None
                }
                // Arrow Up -> If Shift pressed, scroll backward 10 lines; else ANSI "\x1b[A"
                0x48 => {
                    if self.left_shift || self.right_shift {
                        crate::fb_console::scroll_history_backward(10);
                        None
                    } else {
                        crate::fb_console::reset_scroll();
                        let w1 = self.push_byte(0x1B);
                        let w2 = self.push_byte(b'[');
                        let w3 = self.push_byte(b'A');
                        w1.or(w2).or(w3)
                    }
                }
                // Arrow Down -> If Shift pressed, scroll forward 10 lines; else ANSI "\x1b[B"
                0x50 => {
                    if self.left_shift || self.right_shift {
                        crate::fb_console::scroll_history_forward(10);
                        None
                    } else {
                        crate::fb_console::reset_scroll();
                        let w1 = self.push_byte(0x1B);
                        let w2 = self.push_byte(b'[');
                        let w3 = self.push_byte(b'B');
                        w1.or(w2).or(w3)
                    }
                }
                // Arrow Right -> ANSI "\x1b[C"
                0x4D => {
                    crate::fb_console::reset_scroll();
                    let w1 = self.push_byte(0x1B);
                    let w2 = self.push_byte(b'[');
                    let w3 = self.push_byte(b'C');
                    w1.or(w2).or(w3)
                }
                // Arrow Left -> ANSI "\x1b[D"
                0x4B => {
                    crate::fb_console::reset_scroll();
                    let w1 = self.push_byte(0x1B);
                    let w2 = self.push_byte(b'[');
                    let w3 = self.push_byte(b'D');
                    w1.or(w2).or(w3)
                }
                // Home -> ANSI "\x1b[H"
                0x47 => {
                    crate::fb_console::reset_scroll();
                    let w1 = self.push_byte(0x1B);
                    let w2 = self.push_byte(b'[');
                    let w3 = self.push_byte(b'H');
                    w1.or(w2).or(w3)
                }
                // End -> ANSI "\x1b[F"
                0x4F => {
                    crate::fb_console::reset_scroll();
                    let w1 = self.push_byte(0x1B);
                    let w2 = self.push_byte(b'[');
                    let w3 = self.push_byte(b'F');
                    w1.or(w2).or(w3)
                }
                // Delete -> 0x7F (ASCII DEL)
                0x53 => {
                    crate::fb_console::reset_scroll();
                    self.push_byte(0x7F)
                }
                // Left Win (0x5B) / Right Win (0x5C) -> 0x11 (DC1 / Win Super key)
                0x5B | 0x5C => self.push_byte(0x11),
                // Keypad Enter (0x1C with E0)
                0x1C => {
                    crate::fb_console::reset_scroll();
                    self.push_byte(b'\n')
                }
                // Keypad / (0x35 with E0)
                0x35 => {
                    crate::fb_console::reset_scroll();
                    self.push_byte(b'/')
                }
                _ => None,
            };
        }

        match scancode {
            0x2A => {
                self.left_shift = true;
                None
            }
            0xAA => {
                self.left_shift = false;
                None
            }
            0x36 => {
                self.right_shift = true;
                None
            }
            0xB6 => {
                self.right_shift = false;
                None
            }
            0x38 => {
                self.left_alt = true;
                None
            }
            0xB8 => {
                self.left_alt = false;
                None
            }
            0x3A => {
                self.caps_lock = !self.caps_lock;
                None
            }
            code if (code & 0x80) != 0 => None, // Key release (break code)
            code => {
                if self.left_alt || self.right_alt {
                    // Spanish/Latam keyboard Alt shortcuts:
                    // Alt + 1 -> '|'
                    // Alt + 2 -> '>'
                    // Alt + 3 -> '<'
                    // Alt + 4 -> '~'
                    let alt_ch = match code {
                        0x02 => Some(b'|'),
                        0x03 => Some(b'>'),
                        0x04 => Some(b'<'),
                        0x05 => Some(b'~'),
                        _ => None,
                    };
                    if let Some(c) = alt_ch {
                        crate::fb_console::reset_scroll();
                        return self.push_byte(c);
                    }
                }

                let idx = code as usize;
                if idx >= SCANCODE_SET1_NORMAL.len() {
                    return None;
                }
                let shift = self.left_shift || self.right_shift;
                let mut ch = if shift {
                    SCANCODE_SET1_SHIFTED[idx]
                } else {
                    SCANCODE_SET1_NORMAL[idx]
                };

                if self.caps_lock {
                    if ch.is_ascii_lowercase() {
                        ch = ch.to_ascii_uppercase();
                    } else if ch.is_ascii_uppercase() {
                        ch = ch.to_ascii_lowercase();
                    }
                }

                if ch != 0 {
                    crate::fb_console::reset_scroll();
                    self.push_byte(ch)
                } else {
                    None
                }
            }
        }
    }
}

static KEYBOARD: SpinLock<KeyboardState> = SpinLock::new(KeyboardState::new());

/// Drains any pending byte from the PS/2 controller output buffer at boot.
pub fn init() {
    // SAFETY: Reading PS/2 controller status (0x64) and data (0x60) in Ring 0.
    unsafe {
        for _ in 0..16 {
            if (inb(PS2_STATUS_PORT) & 0x01) != 0 {
                let _ = inb(PS2_DATA_PORT);
            } else {
                break;
            }
        }
    }
}

/// Invoked from `__rust_keyboard_irq_handler` (`IRQ1`, Vector `0x21`) when a key event arrives.
pub(crate) fn on_irq1() {
    // SAFETY: Reading PS/2 status (0x64) and data port (0x60) during IRQ1 service routine.
    unsafe {
        let status = inb(PS2_STATUS_PORT);
        if (status & 0x01) == 0 {
            return;
        }
        let byte = inb(PS2_DATA_PORT);
        if (status & 0x20) != 0 {
            super::mouse::process_packet_byte(byte);
            return;
        }
        process_scancode_from_aux_poll(byte);
    }
}

/// Processes a keyboard scancode read from port `0x60` when `status & 0x20 == 0`.
pub(crate) fn process_scancode_from_aux_poll(scancode: u8) {
    let waker = if let Some(mut guard) = KEYBOARD.try_lock() {
        guard.handle_scancode(scancode)
    } else {
        None
    };
    if let Some(w) = waker {
        w.wake();
    }
}

/// Registers a `Waker` to be notified on the next keystroke.
pub fn register_waker(waker: Waker) {
    KEYBOARD.lock().waker = Some(waker);
}

/// Attempts a non-blocking read of a decoded ASCII character from the `IRQ1` keyboard buffer,
/// PCIe xHCI USB HID keyboard, TCP remote console (port 2222), or fallback serial console.
pub fn try_read_char() -> Option<u8> {
    if let Some(ch) = KEYBOARD.lock().pop_byte() {
        return Some(ch);
    }
    if let Some(ch) = crate::bus::xhci::pop_hid_byte() {
        return Some(ch);
    }
    crate::net::tcp::poll_remote_console_tcp();
    if let Some(ch) = crate::net::tcp::pop_remote_console_byte() {
        return Some(ch);
    }
    super::serial::try_read_byte()
}

/// Immediately extracts a single pending key from the `IRQ1` PS/2 ring buffer, USB HID xHCI,
/// TCP remote console, hardware PS/2 controller port (`0x60`), or fallback UART `0x3F8`.
pub fn try_pop_key_nonblocking() -> Option<u8> {
    if let Some(ch) = KEYBOARD.lock().pop_byte() {
        return Some(ch);
    }
    if let Some(ch) = crate::bus::xhci::pop_hid_byte() {
        return Some(ch);
    }
    crate::net::tcp::poll_remote_console_tcp();
    if let Some(ch) = crate::net::tcp::pop_remote_console_byte() {
        return Some(ch);
    }

    // Drain up to 4 pending hardware PS/2 bytes in case a key/mouse byte arrived while IF=0 during a syscall
    // SAFETY: Reading PS/2 status port (0x64) and data port (0x60) in Ring 0.
    unsafe {
        for _ in 0..4 {
            let status = inb(PS2_STATUS_PORT);
            if (status & 0x01) != 0 {
                let data = inb(PS2_DATA_PORT);
                if (status & 0x20) != 0 {
                    super::mouse::process_packet_byte(data);
                } else {
                    let waker = KEYBOARD.lock().handle_scancode(data);
                    if let Some(w) = waker {
                        w.wake();
                    }
                }
            } else {
                break;
            }
        }
    }

    if let Some(ch) = KEYBOARD.lock().pop_byte() {
        return Some(ch);
    }
    if let Some(ch) = crate::bus::xhci::pop_hid_byte() {
        return Some(ch);
    }
    crate::net::tcp::poll_remote_console_tcp();
    if let Some(ch) = crate::net::tcp::pop_remote_console_byte() {
        return Some(ch);
    }

    super::serial::try_read_byte()
}

/// Blocks efficiently using `sti; hlt; cli` until an ASCII character arrives from `IRQ1`
/// (PS/2 Keyboard), USB HID xHCI, TCP Remote Console, or UART COM1.
pub fn read_char_blocking() -> u8 {
    loop {
        crate::net::poll_network_and_remote_console();
        if let Some(ch) = try_read_char() {
            return ch;
        }

        // Idle Power Governor: record idle tick on Core 0 while sleeping in HLT
        crate::scheduler::record_idle_tick(0);
        // SAFETY: Enabling interrupts and halting the CPU core until the next hardware IRQ
        // (`IRQ1` keyboard or `IRQ0` timer) wakes the processor.
        unsafe {
            core::arch::asm!("sti", "hlt", "cli", options(nomem, nostack));
        }

        crate::net::poll_network_and_remote_console();
    }
}

#[inline]
unsafe fn inb(port: u16) -> u8 {
    let value: u8;
    // SAFETY: Caller guarantees valid I/O port.
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
