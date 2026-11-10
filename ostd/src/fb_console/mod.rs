use core::fmt::{self, Write};
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use crate::sync::SpinLock;
use crate::boot::BOOT_FB_COLOR_RGB;

pub mod ansi;
pub mod dma;
pub mod render;
pub mod scrollback;

pub use ansi::*;
pub use dma::*;
pub(crate) use render::*;
pub use scrollback::*;

pub(crate) static LOCKFREE_FB_ADDR: AtomicUsize = AtomicUsize::new(0);
pub(crate) static LOCKFREE_FB_WIDTH: AtomicUsize = AtomicUsize::new(0);
pub(crate) static LOCKFREE_FB_HEIGHT: AtomicUsize = AtomicUsize::new(0);
pub(crate) static LOCKFREE_FB_PITCH_PIXELS: AtomicUsize = AtomicUsize::new(0);
pub(crate) static DESKTOP_MODE: AtomicBool = AtomicBool::new(false);

pub struct FramebufferConsole {
    fb_addr: usize,
    width: usize,
    height: usize,
    pitch_pixels: usize,
    bpp: usize,
    cursor_col: usize,
    cursor_row: usize,
    max_cols: usize,
    max_rows: usize,
    fg_color: u32,
    history: [ConsoleLine; SCROLLBACK_LINES],
    history_total: usize,
    pub scroll_offset: usize,
    current_line: ConsoleLine,
    ansi_state: AnsiState,
    ansi_buf: [u8; 32],
    ansi_len: usize,
}

impl FramebufferConsole {
    pub const fn new() -> Self {
        Self {
            fb_addr: 0,
            width: 0,
            height: 0,
            pitch_pixels: 0,
            bpp: 0,
            cursor_col: 0,
            cursor_row: 0,
            max_cols: 0,
            max_rows: 0,
            fg_color: COLOR_TEXT_DEFAULT,
            history: [ConsoleLine::empty(); SCROLLBACK_LINES],
            history_total: 0,
            scroll_offset: 0,
            current_line: ConsoleLine::empty(),
            ansi_state: AnsiState::Normal,
            ansi_buf: [0; 32],
            ansi_len: 0,
        }
    }

    fn init(&mut self, fb_ptr: *mut u8, width: usize, height: usize, pitch: usize, bpp: usize) {
        if bpp != 32 || width < 320 || height < 200 {
            return;
        }
        self.fb_addr = fb_ptr as usize;
        self.width = width;
        self.height = height;
        self.pitch_pixels = pitch / 4;
        self.bpp = bpp;
        self.cursor_col = 0;
        self.cursor_row = 0;
        self.max_cols = (width.saturating_sub(TEXT_LEFT_X * 2)) / CHAR_WIDTH;
        self.max_rows = (height.saturating_sub(TEXT_TOP_Y + 8)) / LINE_HEIGHT;
        self.fg_color = COLOR_TEXT_DEFAULT;
        self.scroll_offset = 0;
        self.ansi_state = AnsiState::Normal;
        self.ansi_len = 0;

        self.draw_header_banner();
    }

    #[inline]

    fn reset_console_surface(&mut self) {
        if self.fb_addr == 0 {
            return;
        }
        self.fill_rect(0, 0, self.width, self.height, BOOT_FB_COLOR_RGB);
        self.cursor_col = 0;
        self.cursor_row = 0;
        self.scroll_offset = 0;
        self.fg_color = COLOR_TEXT_DEFAULT;
        self.draw_header_banner();
    }

    fn newline(&mut self) {
        self.push_current_line_to_history();
        self.cursor_col = 0;
        self.cursor_row += 1;
        self.fg_color = COLOR_TEXT_DEFAULT;

        if self.cursor_row >= self.max_rows && self.max_rows > 0 {
            self.scroll_up_one_line();
            self.cursor_row = self.max_rows - 1;
        }
    }


    pub fn write_text(&mut self, s: &str) {
        if self.fb_addr == 0 || self.max_cols == 0 || self.max_rows == 0 {
            return;
        }

        crate::net::tcp::remote_console_broadcast(s.as_bytes());

        if self.scroll_offset > 0 {
            self.scroll_offset = 0;
            self.redraw_view();
        }

        if self.cursor_col == 0 {
            self.auto_select_line_color(s);
        }

        for b in s.bytes() {
            match self.ansi_state {
                AnsiState::Normal => {
                    match b {
                        0x1B => {
                            self.ansi_state = AnsiState::Esc;
                            self.ansi_len = 0;
                        }
                        b'\n' => self.newline(),
                        b'\r' => self.cursor_col = 0,
                        0x08 | 0x7F => {
                            if self.cursor_col > 0 {
                                self.cursor_col -= 1;
                                if self.cursor_col < MAX_LINE_COLS {
                                    self.current_line.cells[self.cursor_col] = ConsoleCell::empty();
                                    if self.cursor_col == self.current_line.len.saturating_sub(1) {
                                        self.current_line.len = self.cursor_col;
                                    }
                                }
                                let px_x = TEXT_LEFT_X + self.cursor_col * CHAR_WIDTH;
                                let px_y = TEXT_TOP_Y + self.cursor_row * LINE_HEIGHT;
                                self.fill_rect(px_x, px_y, CHAR_WIDTH, LINE_HEIGHT, BOOT_FB_COLOR_RGB);
                            }
                        }
                        b'\t' => {
                            let next_col = ((self.cursor_col + 8) & !7).min(self.max_cols.saturating_sub(1));
                            while self.cursor_col < next_col {
                                self.cursor_col += 1;
                            }
                        }
                        ch if (0x20..=0x7E).contains(&ch) => {
                            if self.cursor_col >= self.max_cols {
                                let keep_color = self.fg_color;
                                self.newline();
                                self.fg_color = keep_color;
                            }
                            if self.cursor_col < MAX_LINE_COLS {
                                self.current_line.cells[self.cursor_col] = ConsoleCell {
                                    ch,
                                    fg: self.fg_color,
                                };
                                if self.cursor_col + 1 > self.current_line.len {
                                    self.current_line.len = self.cursor_col + 1;
                                }
                            }
                            let px_x = TEXT_LEFT_X + self.cursor_col * CHAR_WIDTH;
                            let px_y = TEXT_TOP_Y + self.cursor_row * LINE_HEIGHT;
                            self.draw_glyph_scaled(
                                ch,
                                px_x,
                                px_y,
                                self.fg_color,
                                Some(BOOT_FB_COLOR_RGB),
                                1,
                            );
                            self.cursor_col += 1;
                        }
                        _ => {
                            // Non-printable control characters are ignored without drawing '?'
                        }
                    }
                }
                AnsiState::Esc => {
                    if b == b'[' {
                        self.ansi_state = AnsiState::Csi;
                        self.ansi_len = 0;
                    } else {
                        self.ansi_state = AnsiState::Normal;
                    }
                }
                AnsiState::Csi => {
                    if (b'0'..=b'9').contains(&b) || b == b';' || b == b'?' {
                        if self.ansi_len < self.ansi_buf.len() {
                            self.ansi_buf[self.ansi_len] = b;
                            self.ansi_len += 1;
                        }
                    } else if (0x40..=0x7E).contains(&b) {
                        self.execute_csi_command(b);
                        self.ansi_state = AnsiState::Normal;
                        self.ansi_len = 0;
                    } else {
                        self.ansi_state = AnsiState::Normal;
                        self.ansi_len = 0;
                    }
                }
            }
        }
    }
}

impl Write for FramebufferConsole {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.write_text(s);
        Ok(())
    }
}

pub(crate) static FB_CONSOLE: SpinLock<FramebufferConsole> = SpinLock::new(FramebufferConsole::new());

/// Initializes the global visual Framebuffer console on the Limine linear framebuffer.
pub fn init(fb_ptr: *mut u8, width: usize, height: usize, pitch: usize, bpp: usize) {
    if bpp == 32 {
        LOCKFREE_FB_ADDR.store(fb_ptr as usize, Ordering::Release);
        LOCKFREE_FB_WIDTH.store(width, Ordering::Release);
        LOCKFREE_FB_HEIGHT.store(height, Ordering::Release);
        LOCKFREE_FB_PITCH_PIXELS.store(pitch / 4, Ordering::Release);
    }
    FB_CONSOLE.lock().init(fb_ptr, width, height, pitch, bpp);
}

/// Scrolls the framebuffer console view backward (towards past history) by `lines` rows.
pub fn scroll_history_backward(lines: usize) {
    FB_CONSOLE.lock().scroll_history_backward(lines);
}

/// Scrolls the framebuffer console view forward (towards active live output) by `lines` rows.
pub fn scroll_history_forward(lines: usize) {
    FB_CONSOLE.lock().scroll_history_forward(lines);
}

/// Resets console scroll offset to 0 (live view) and redraws the active terminal screen.
pub fn reset_scroll() {
    FB_CONSOLE.lock().reset_scroll();
}

/// Executes closure `f` with exclusive access to the global `FramebufferConsole`.
/// If the lock is held during an emergency panic, it is forcibly released to ensure diagnostics display.
pub fn with_console<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&mut FramebufferConsole) -> R,
{
    if let Some(mut guard) = FB_CONSOLE.try_lock() {
        Some(f(&mut *guard))
    } else {
        // SAFETY: Only used during emergency exception/panic when lock cannot be acquired normally.
        unsafe {
            FB_CONSOLE.force_unlock();
        }
        let mut guard = FB_CONSOLE.lock();
        Some(f(&mut *guard))
    }
}

/// Initializes or prepares the high-speed console backbuffer.

pub fn dimensions() -> (usize, usize) {
    let w = LOCKFREE_FB_WIDTH.load(Ordering::Acquire);
    let h = LOCKFREE_FB_HEIGHT.load(Ordering::Acquire);
    if w >= 320 && h >= 200 {
        (w, h)
    } else {
        (1280, 800)
    }
}

/// Returns the active linear framebuffer layout `(width, height, pitch_bytes, bpp)`.
pub fn framebuffer_info() -> (usize, usize, usize, usize) {
    let w = LOCKFREE_FB_WIDTH.load(Ordering::Acquire);
    let h = LOCKFREE_FB_HEIGHT.load(Ordering::Acquire);
    let pitch = LOCKFREE_FB_PITCH_PIXELS.load(Ordering::Acquire) * 4;
    let bpp = 32;
    if w >= 320 && h >= 200 {
        (w, h, pitch, bpp)
    } else {
        (1280, 800, 5120, 32)
    }
}


pub fn set_desktop_mode(active: bool) {
    let prev = DESKTOP_MODE.swap(active, Ordering::AcqRel);
    {
        let mut overlay = MOUSE_OVERLAY.lock();
        overlay.has_prev = false;
    }
    if !active && prev {
        FB_CONSOLE.lock().reset_console_surface();
        #[cfg(target_arch = "x86_64")]
        {
            let snap = crate::arch::mouse::poll_state();
            draw_mouse_cursor_overlay(snap.x as usize, snap.y as usize, snap.buttons);
        }
    }
}

/// Returns `true` if the linear framebuffer console has been initialized with a valid physical address.
pub fn is_active() -> bool {
    LOCKFREE_FB_ADDR.load(Ordering::Relaxed) != 0
}

/// Returns `true` when the Ring 3 `1280x800` Desktop Compositor owns `/dev/fb0`.
#[inline]
pub fn is_desktop_mode() -> bool {
    DESKTOP_MODE.load(Ordering::Acquire)
}

/// Zero-Copy Double-Buffer DMA Flip: transfers rectangle `(x, y, w, h)` directly from the physical
/// `/dev/fb0` `DmaBuffer` Backbuffer (`1280x800x4`) in HHDM to the Limine hardware Frontbuffer
/// with zero heap allocations and race-free hardware PS/2 cursor restoration.

pub fn _print(args: fmt::Arguments<'_>) {
    if DESKTOP_MODE.load(Ordering::Acquire) {
        return;
    }
    let mut guard = FB_CONSOLE.lock();
    let _ = guard.write_fmt(args);
}
