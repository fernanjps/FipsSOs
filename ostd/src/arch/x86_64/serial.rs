use core::fmt::{self, Write};
use core::sync::atomic::{AtomicBool, Ordering};

/// Base I/O port for the primary UART 16550 serial controller (COM1).
pub const COM1_BASE_PORT: u16 = 0x3F8;

static SERIAL_INITIALIZED: AtomicBool = AtomicBool::new(false);
static SERIAL_LOCK: AtomicBool = AtomicBool::new(false);

/// Safe wrapper around an x86_64 UART 16550 serial port.
pub struct SerialPort {
    base_port: u16,
}

impl SerialPort {
    /// Creates a handle for the COM1 (`0x3F8`) UART 16550 port.
    pub const fn new_com1() -> Self {
        Self {
            base_port: COM1_BASE_PORT,
        }
    }

    /// Initializes the UART 16550 controller (38400 baud, 8N1, FIFO enabled).
    pub fn init(&mut self) {
        // SAFETY: Accessing standard x86_64 COM1 I/O ports (0x3F8..0x3FD) in Ring 0.
        unsafe {
            // Disable all UART interrupts
            outb(self.base_port + 1, 0x00);
            // Enable DLAB (Divisor Latch Access Bit) to set baud rate divisor
            outb(self.base_port + 3, 0x80);
            // Set divisor to 3 (lo byte = 0x03, hi byte = 0x00) -> 38400 baud
            outb(self.base_port, 0x03);
            outb(self.base_port + 1, 0x00);
            // 8 bits, no parity, one stop bit (8N1), clear DLAB
            outb(self.base_port + 3, 0x03);
            // Enable FIFO, clear TX/RX queues, 14-byte interrupt threshold
            outb(self.base_port + 2, 0xC7);
            // Auxiliary output 2 (IRQ enable), RTS and DTR set
            outb(self.base_port + 4, 0x0B);
        }
    }

    /// Returns `true` if the UART Transmit Holding Register is empty (bit 5 of LSR).
    #[inline]
    fn is_transmit_empty(&self) -> bool {
        // SAFETY: Reading Line Status Register (base + 5) of COM1 is side-effect free.
        unsafe { (inb(self.base_port + 5) & 0x20) != 0 }
    }

    /// Writes a single byte to the serial port, waiting until the TX buffer is ready.
    pub fn send_byte(&mut self, byte: u8) {
        if byte == b'\n' {
            self.send_raw_byte(b'\r');
        }
        self.send_raw_byte(byte);
    }

    #[inline]
    fn send_raw_byte(&mut self, byte: u8) {
        while !self.is_transmit_empty() {
            core::hint::spin_loop();
        }
        // SAFETY: Writing to the UART Data Register (base + 0) when THRE is set.
        unsafe {
            outb(self.base_port, byte);
        }
    }
}

impl Write for SerialPort {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for byte in s.bytes() {
            self.send_byte(byte);
        }
        Ok(())
    }
}

/// Initializes the global COM1 UART 16550 serial port if not already initialized.
pub fn init() {
    if !SERIAL_INITIALIZED.swap(true, Ordering::AcqRel) {
        let mut port = SerialPort::new_com1();
        port.init();
    }
}

/// Internal formatting entrypoint used by `ostd::early_print!` and `ostd::early_println!`.
#[doc(hidden)]
pub fn _print(args: fmt::Arguments<'_>) {
    init();

    // Acquire simple atomic spinlock to prevent interleaved serial output
    while SERIAL_LOCK
        .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        core::hint::spin_loop();
    }

    let mut port = SerialPort::new_com1();
    let _ = port.write_fmt(args);

    SERIAL_LOCK.store(false, Ordering::Release);
}

/// Attempts a non-blocking read of a single byte from the COM1 (`0x3F8`) receive buffer.
pub fn try_read_byte() -> Option<u8> {
    init();
    // SAFETY: Reading Line Status Register (0x3FD) bit 0 (Data Ready) and Data Register (0x3F8).
    unsafe {
        if (inb(COM1_BASE_PORT + 5) & 0x01) != 0 {
            let b = inb(COM1_BASE_PORT);
            Some(match b {
                b'\r' => b'\n',
                0x7F => 0x08,
                other => other,
            })
        } else {
            None
        }
    }
}

#[inline]
unsafe fn outb(port: u16, value: u8) {
    // SAFETY: Caller guarantees the I/O port is valid for 8-bit output.
    unsafe {
        core::arch::asm!(
            "out dx, al",
            in("dx") port,
            in("al") value,
            options(nomem, nostack, preserves_flags)
        );
    }
}

#[inline]
unsafe fn inb(port: u16) -> u8 {
    let value: u8;
    // SAFETY: Caller guarantees the I/O port is valid for 8-bit input.
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
