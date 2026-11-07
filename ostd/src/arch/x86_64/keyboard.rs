use core::task::Waker;

use crate::sync::SpinLock;

/// PS/2 Controller I/O ports.
pub const PS2_DATA_PORT: u16 = 0x60;
pub const PS2_STATUS_PORT: u16 = 0x64;

const KEY_RING_CAPACITY: usize = 256;

/// Scancode Set 1 unshifted lookup table for make codes `0x00..0x58`.
static SCANCODE_SET1_NORMAL: [u8; 89] = [
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
    0,                                              // 0x58
];

/// Scancode Set 1 shifted lookup table for make codes `0x00..0x58`.
static SCANCODE_SET1_SHIFTED: [u8; 89] = [
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
    0,                                              // 0x58
];

struct KeyboardState {
    buf: [u8; KEY_RING_CAPACITY],
    head: usize,
    tail: usize,
    len: usize,
    left_shift: bool,
    right_shift: bool,
    caps_lock: bool,
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
            caps_lock: false,
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
        match scancode {
            0xE0 => None, // Extended prefix
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
            0x3A => {
                self.caps_lock = !self.caps_lock;
                None
            }
            code if (code & 0x80) != 0 => None, // Key release (break code)
            code => {
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
    // SAFETY: Reading scancode from PS/2 data port 0x60 during IRQ1 service routine.
    let scancode = unsafe { inb(PS2_DATA_PORT) };
    let waker = KEYBOARD.lock().handle_scancode(scancode);
    if let Some(w) = waker {
        w.wake();
    }
}

/// Registers a `Waker` to be notified on the next keystroke.
pub fn register_waker(waker: Waker) {
    KEYBOARD.lock().waker = Some(waker);
}

/// Attempts a non-blocking read of a decoded ASCII character from the `IRQ1` keyboard buffer
/// (or fallback serial console).
pub fn try_read_char() -> Option<u8> {
    if let Some(ch) = KEYBOARD.lock().pop_byte() {
        return Some(ch);
    }
    super::serial::try_read_byte()
}

/// Immediately extracts a single pending key from the `IRQ1` PS/2 ring buffer, hardware PS/2
/// controller port (`0x60`), or fallback UART `0x3F8` receiver without waiting for `\n` and
/// without blocking if no key is pressed (Real-Time Key Polling for 3D engines/games).
pub fn try_pop_key_nonblocking() -> Option<u8> {
    if let Some(ch) = KEYBOARD.lock().pop_byte() {
        return Some(ch);
    }

    // Drain up to 4 pending hardware PS/2 bytes in case a key was pressed while IF=0 during a syscall
    // SAFETY: Reading PS/2 status port (0x64) and data port (0x60) in Ring 0.
    unsafe {
        for _ in 0..4 {
            if (inb(PS2_STATUS_PORT) & 0x01) != 0 {
                let scancode = inb(PS2_DATA_PORT);
                let waker = KEYBOARD.lock().handle_scancode(scancode);
                if let Some(w) = waker {
                    w.wake();
                }
            } else {
                break;
            }
        }
    }

    if let Some(ch) = KEYBOARD.lock().pop_byte() {
        return Some(ch);
    }

    super::serial::try_read_byte()
}

/// Blocks efficiently using `sti; hlt; cli` until an ASCII character arrives from `IRQ1`
/// (PS/2 Keyboard) or UART COM1.
pub fn read_char_blocking() -> u8 {
    loop {
        if let Some(ch) = try_read_char() {
            return ch;
        }
        // SAFETY: Enabling interrupts and halting the CPU core until the next hardware IRQ
        // (`IRQ1` keyboard or `IRQ0` timer) wakes the processor.
        unsafe {
            core::arch::asm!("sti", "hlt", "cli", options(nomem, nostack));
        }
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
