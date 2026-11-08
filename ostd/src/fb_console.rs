use core::fmt::{self, Write};
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crate::boot::BOOT_FB_COLOR_RGB;
use crate::mm::DmaBuffer;
use crate::sync::SpinLock;

static LOCKFREE_FB_ADDR: AtomicUsize = AtomicUsize::new(0);
static LOCKFREE_FB_WIDTH: AtomicUsize = AtomicUsize::new(0);
static LOCKFREE_FB_HEIGHT: AtomicUsize = AtomicUsize::new(0);
static LOCKFREE_FB_PITCH_PIXELS: AtomicUsize = AtomicUsize::new(0);
static DESKTOP_MODE: AtomicBool = AtomicBool::new(false);

/// Header banner height in pixels at the top of the linear framebuffer.
const HEADER_HEIGHT: usize = 48;
/// Top Y coordinate where scrollable console text begins.
const TEXT_TOP_Y: usize = 56;
/// Left X margin in pixels.
const TEXT_LEFT_X: usize = 12;
/// Horizontal advance per character cell (8 pixels).
const CHAR_WIDTH: usize = 8;
/// Vertical advance per text row (14 pixels: 8px glyph + 6px leading).
const LINE_HEIGHT: usize = 14;

/// Color palette for the FipSOs visual Framebuffer console (32-bit `0x00RRGGBB`).
const COLOR_HEADER_BG: u32 = 0x000D_1526;
const COLOR_HEADER_BORDER: u32 = 0x0000_F5D4;
const COLOR_HEADER_TITLE: u32 = 0x0000_F5D4;
const COLOR_HEADER_SUBTITLE: u32 = 0x00FF_D166;

const COLOR_TEXT_DEFAULT: u32 = 0x00E6_F1FF;
const COLOR_TEXT_OSTD: u32 = 0x005C_E1E6;
const COLOR_TEXT_FIPSOS: u32 = 0x007A_E582;
const COLOR_TEXT_LINUX_ABI: u32 = 0x00FF_D166;
const COLOR_TEXT_ASYNC: u32 = 0x00C7_7DFF;
const COLOR_TEXT_RING3: u32 = 0x00FF_9E00;
const COLOR_TEXT_SCHEDULER: u32 = 0x0000_F5D4;

/// Embedded 8x8 bitmap font covering printable ASCII `0x20..=0x7E` (95 glyphs).
/// Each row byte has MSB (`0x80`) as the leftmost pixel and LSB (`0x01`) as the rightmost pixel.
static FONT_8X8: [[u8; 8]; 95] = [
    [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], // 0x20 ' '
    [0x18, 0x3C, 0x3C, 0x18, 0x18, 0x00, 0x18, 0x00], // 0x21 '!'
    [0x6C, 0x6C, 0x48, 0x00, 0x00, 0x00, 0x00, 0x00], // 0x22 '"'
    [0x6C, 0x6C, 0xFE, 0x6C, 0xFE, 0x6C, 0x6C, 0x00], // 0x23 '#'
    [0x18, 0x3E, 0x60, 0x3C, 0x06, 0x7C, 0x18, 0x00], // 0x24 '$'
    [0x00, 0xC6, 0xCC, 0x18, 0x30, 0x66, 0xC6, 0x00], // 0x25 '%'
    [0x38, 0x6C, 0x38, 0x76, 0xDC, 0xCC, 0x76, 0x00], // 0x26 '&'
    [0x18, 0x18, 0x30, 0x00, 0x00, 0x00, 0x00, 0x00], // 0x27 '\''
    [0x0C, 0x18, 0x30, 0x30, 0x30, 0x18, 0x0C, 0x00], // 0x28 '('
    [0x30, 0x18, 0x0C, 0x0C, 0x0C, 0x18, 0x30, 0x00], // 0x29 ')'
    [0x00, 0x66, 0x3C, 0xFF, 0x3C, 0x66, 0x00, 0x00], // 0x2A '*'
    [0x00, 0x18, 0x18, 0x7E, 0x18, 0x18, 0x00, 0x00], // 0x2B '+'
    [0x00, 0x00, 0x00, 0x00, 0x00, 0x18, 0x18, 0x30], // 0x2C ','
    [0x00, 0x00, 0x00, 0x7E, 0x00, 0x00, 0x00, 0x00], // 0x2D '-'
    [0x00, 0x00, 0x00, 0x00, 0x00, 0x18, 0x18, 0x00], // 0x2E '.'
    [0x06, 0x0C, 0x18, 0x30, 0x60, 0xC0, 0x80, 0x00], // 0x2F '/'
    [0x7C, 0xC6, 0xCE, 0xDE, 0xF6, 0xE6, 0x7C, 0x00], // 0x30 '0'
    [0x18, 0x38, 0x18, 0x18, 0x18, 0x18, 0x7E, 0x00], // 0x31 '1'
    [0x7C, 0xC6, 0x06, 0x1C, 0x30, 0x66, 0xFE, 0x00], // 0x32 '2'
    [0x7C, 0xC6, 0x06, 0x3C, 0x06, 0xC6, 0x7C, 0x00], // 0x33 '3'
    [0x1C, 0x3C, 0x6C, 0xCC, 0xFE, 0x0C, 0x1E, 0x00], // 0x34 '4'
    [0xFE, 0xC0, 0xFC, 0x06, 0x06, 0xC6, 0x7C, 0x00], // 0x35 '5'
    [0x38, 0x60, 0xC0, 0xFC, 0xC6, 0xC6, 0x7C, 0x00], // 0x36 '6'
    [0xFE, 0xC6, 0x0C, 0x18, 0x30, 0x30, 0x30, 0x00], // 0x37 '7'
    [0x7C, 0xC6, 0xC6, 0x7C, 0xC6, 0xC6, 0x7C, 0x00], // 0x38 '8'
    [0x7C, 0xC6, 0xC6, 0x7E, 0x06, 0x0C, 0x78, 0x00], // 0x39 '9'
    [0x00, 0x18, 0x18, 0x00, 0x00, 0x18, 0x18, 0x00], // 0x3A ':'
    [0x00, 0x18, 0x18, 0x00, 0x00, 0x18, 0x18, 0x30], // 0x3B ';'
    [0x0C, 0x18, 0x30, 0x60, 0x30, 0x18, 0x0C, 0x00], // 0x3C '<'
    [0x00, 0x00, 0x7E, 0x00, 0x00, 0x7E, 0x00, 0x00], // 0x3D '='
    [0x60, 0x30, 0x18, 0x0C, 0x18, 0x30, 0x60, 0x00], // 0x3E '>'
    [0x7C, 0xC6, 0x0C, 0x18, 0x18, 0x00, 0x18, 0x00], // 0x3F '?'
    [0x7C, 0xC6, 0xDE, 0xDE, 0xDE, 0xC0, 0x78, 0x00], // 0x40 '@'
    [0x38, 0x6C, 0xC6, 0xFE, 0xC6, 0xC6, 0xC6, 0x00], // 0x41 'A'
    [0xFC, 0x66, 0x66, 0x7C, 0x66, 0x66, 0xFC, 0x00], // 0x42 'B'
    [0x3C, 0x66, 0xC0, 0xC0, 0xC0, 0x66, 0x3C, 0x00], // 0x43 'C'
    [0xF8, 0x6C, 0x66, 0x66, 0x66, 0x6C, 0xF8, 0x00], // 0x44 'D'
    [0xFE, 0x62, 0x68, 0x78, 0x68, 0x62, 0xFE, 0x00], // 0x45 'E'
    [0xFE, 0x62, 0x68, 0x78, 0x68, 0x60, 0xF0, 0x00], // 0x46 'F'
    [0x3C, 0x66, 0xC0, 0xC0, 0xCE, 0x66, 0x3E, 0x00], // 0x47 'G'
    [0xC6, 0xC6, 0xC6, 0xFE, 0xC6, 0xC6, 0xC6, 0x00], // 0x48 'H'
    [0x3C, 0x18, 0x18, 0x18, 0x18, 0x18, 0x3C, 0x00], // 0x49 'I'
    [0x1E, 0x0C, 0x0C, 0x0C, 0xCC, 0xCC, 0x78, 0x00], // 0x4A 'J'
    [0xE6, 0x66, 0x6C, 0x78, 0x6C, 0x66, 0xE6, 0x00], // 0x4B 'K'
    [0xF0, 0x60, 0x60, 0x60, 0x62, 0x66, 0xFE, 0x00], // 0x4C 'L'
    [0xC6, 0xEE, 0xFE, 0xFE, 0xD6, 0xC6, 0xC6, 0x00], // 0x4D 'M'
    [0xC6, 0xE6, 0xF6, 0xDE, 0xCE, 0xC6, 0xC6, 0x00], // 0x4E 'N'
    [0x7C, 0xC6, 0xC6, 0xC6, 0xC6, 0xC6, 0x7C, 0x00], // 0x4F 'O'
    [0xFC, 0x66, 0x66, 0x7C, 0x60, 0x60, 0xF0, 0x00], // 0x50 'P'
    [0x7C, 0xC6, 0xC6, 0xC6, 0xD6, 0xDE, 0x7C, 0x0E], // 0x51 'Q'
    [0xFC, 0x66, 0x66, 0x7C, 0x6C, 0x66, 0xE6, 0x00], // 0x52 'R'
    [0x7C, 0xC6, 0x60, 0x38, 0x0C, 0xC6, 0x7C, 0x00], // 0x53 'S'
    [0x7E, 0x7E, 0x5A, 0x18, 0x18, 0x18, 0x3C, 0x00], // 0x54 'T'
    [0xC6, 0xC6, 0xC6, 0xC6, 0xC6, 0xC6, 0x7C, 0x00], // 0x55 'U'
    [0xC6, 0xC6, 0xC6, 0xC6, 0xC6, 0x6C, 0x38, 0x00], // 0x56 'V'
    [0xC6, 0xC6, 0xC6, 0xD6, 0xFE, 0xEE, 0xC6, 0x00], // 0x57 'W'
    [0xC6, 0xC6, 0x6C, 0x38, 0x6C, 0xC6, 0xC6, 0x00], // 0x58 'X'
    [0x66, 0x66, 0x66, 0x3C, 0x18, 0x18, 0x3C, 0x00], // 0x59 'Y'
    [0xFE, 0xC6, 0x8C, 0x18, 0x32, 0x66, 0xFE, 0x00], // 0x5A 'Z'
    [0x3C, 0x30, 0x30, 0x30, 0x30, 0x30, 0x3C, 0x00], // 0x5B '['
    [0xC0, 0x60, 0x30, 0x18, 0x0C, 0x06, 0x02, 0x00], // 0x5C '\\'
    [0x3C, 0x0C, 0x0C, 0x0C, 0x0C, 0x0C, 0x3C, 0x00], // 0x5D ']'
    [0x10, 0x38, 0x6C, 0xC6, 0x00, 0x00, 0x00, 0x00], // 0x5E '^'
    [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xFF], // 0x5F '_'
    [0x30, 0x18, 0x0C, 0x00, 0x00, 0x00, 0x00, 0x00], // 0x60 '`'
    [0x00, 0x00, 0x78, 0x0C, 0x7C, 0xCC, 0x76, 0x00], // 0x61 'a'
    [0xE0, 0x60, 0x7C, 0x66, 0x66, 0x66, 0xDC, 0x00], // 0x62 'b'
    [0x00, 0x00, 0x7C, 0xC6, 0xC0, 0xC6, 0x7C, 0x00], // 0x63 'c'
    [0x1C, 0x0C, 0x7C, 0xCC, 0xCC, 0xCC, 0x76, 0x00], // 0x64 'd'
    [0x00, 0x00, 0x7C, 0xC6, 0xFE, 0xC0, 0x7C, 0x00], // 0x65 'e'
    [0x3C, 0x66, 0x60, 0xF8, 0x60, 0x60, 0xF0, 0x00], // 0x66 'f'
    [0x00, 0x00, 0x76, 0xCC, 0xCC, 0x7C, 0x0C, 0xF8], // 0x67 'g'
    [0xE0, 0x60, 0x6C, 0x76, 0x66, 0x66, 0xE6, 0x00], // 0x68 'h'
    [0x18, 0x00, 0x38, 0x18, 0x18, 0x18, 0x3C, 0x00], // 0x69 'i'
    [0x06, 0x00, 0x06, 0x06, 0x06, 0x66, 0x66, 0x3C], // 0x6A 'j'
    [0xE0, 0x60, 0x66, 0x6C, 0x78, 0x6C, 0xE6, 0x00], // 0x6B 'k'
    [0x38, 0x18, 0x18, 0x18, 0x18, 0x18, 0x3C, 0x00], // 0x6C 'l'
    [0x00, 0x00, 0xEC, 0xFE, 0xD6, 0xC6, 0xC6, 0x00], // 0x6D 'm'
    [0x00, 0x00, 0xDC, 0x66, 0x66, 0x66, 0x66, 0x00], // 0x6E 'n'
    [0x00, 0x00, 0x7C, 0xC6, 0xC6, 0xC6, 0x7C, 0x00], // 0x6F 'o'
    [0x00, 0x00, 0xDC, 0x66, 0x66, 0x7C, 0x60, 0xF0], // 0x70 'p'
    [0x00, 0x00, 0x76, 0xCC, 0xCC, 0x7C, 0x0C, 0x1E], // 0x71 'q'
    [0x00, 0x00, 0xDC, 0x76, 0x60, 0x60, 0xF0, 0x00], // 0x72 'r'
    [0x00, 0x00, 0x7E, 0xC0, 0x7C, 0x06, 0xFC, 0x00], // 0x73 's'
    [0x30, 0x30, 0xFC, 0x30, 0x30, 0x36, 0x1C, 0x00], // 0x74 't'
    [0x00, 0x00, 0xCC, 0xCC, 0xCC, 0xCC, 0x76, 0x00], // 0x75 'u'
    [0x00, 0x00, 0xC6, 0xC6, 0xC6, 0x6C, 0x38, 0x00], // 0x76 'v'
    [0x00, 0x00, 0xC6, 0xD6, 0xFE, 0xFE, 0x6C, 0x00], // 0x77 'w'
    [0x00, 0x00, 0xC6, 0x6C, 0x38, 0x6C, 0xC6, 0x00], // 0x78 'x'
    [0x00, 0x00, 0xC6, 0xC6, 0xCE, 0x76, 0x06, 0x7C], // 0x79 'y'
    [0x00, 0x00, 0xFE, 0x8C, 0x18, 0x32, 0xFE, 0x00], // 0x7A 'z'
    [0x0E, 0x18, 0x18, 0x70, 0x18, 0x18, 0x0E, 0x00], // 0x7B '{'
    [0x18, 0x18, 0x18, 0x00, 0x18, 0x18, 0x18, 0x00], // 0x7C '|'
    [0x70, 0x18, 0x18, 0x0E, 0x18, 0x18, 0x70, 0x00], // 0x7D '}'
    [0x76, 0xDC, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], // 0x7E '~'
];

/// Thread-safe linear framebuffer text console state.
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

        self.draw_header_banner();
    }

    #[inline]
    fn put_pixel(&self, x: usize, y: usize, color: u32) {
        if self.fb_addr == 0 || x >= self.width || y >= self.height {
            return;
        }
        let offset = y * self.pitch_pixels + x;
        // SAFETY: `fb_addr` points to the Limine-mapped 32-bpp linear framebuffer in HHDM
        // and `(x, y)` is bounds-checked against `(width, height)`.
        unsafe {
            let ptr = (self.fb_addr as *mut u32).add(offset);
            core::ptr::write_volatile(ptr, color);
        }
    }

    fn fill_rect(&self, x0: usize, y0: usize, w: usize, h: usize, color: u32) {
        if self.fb_addr == 0 {
            return;
        }
        let x_end = (x0 + w).min(self.width);
        let y_end = (y0 + h).min(self.height);
        for y in y0..y_end {
            let row_base = y * self.pitch_pixels;
            for x in x0..x_end {
                // SAFETY: Bounds-checked within `(width, height)`.
                unsafe {
                    let ptr = (self.fb_addr as *mut u32).add(row_base + x);
                    core::ptr::write_volatile(ptr, color);
                }
            }
        }
    }

    fn draw_glyph_scaled(
        &self,
        ch: u8,
        px_x: usize,
        px_y: usize,
        fg: u32,
        bg: Option<u32>,
        scale: usize,
    ) {
        let idx = if (0x20..=0x7E).contains(&ch) {
            (ch - 0x20) as usize
        } else {
            (b'?' - 0x20) as usize
        };
        let glyph = &FONT_8X8[idx];

        for (gy, &row_bits) in glyph.iter().enumerate() {
            for gx in 0..8 {
                let bit_set = (row_bits & (0x80 >> gx)) != 0;
                if bit_set {
                    for sy in 0..scale {
                        for sx in 0..scale {
                            self.put_pixel(px_x + gx * scale + sx, px_y + gy * scale + sy, fg);
                        }
                    }
                } else if let Some(bg_col) = bg {
                    for sy in 0..scale {
                        for sx in 0..scale {
                            self.put_pixel(px_x + gx * scale + sx, px_y + gy * scale + sy, bg_col);
                        }
                    }
                }
            }
        }
    }

    fn draw_str_at(&self, text: &str, mut x: usize, y: usize, fg: u32, scale: usize) {
        for b in text.bytes() {
            self.draw_glyph_scaled(b, x, y, fg, None, scale);
            x += 8 * scale;
        }
    }

    /// Paints the top visual FipSOs header bar on the linear framebuffer.
    fn draw_header_banner(&self) {
        self.fill_rect(0, 0, self.width, HEADER_HEIGHT - 2, COLOR_HEADER_BG);
        self.fill_rect(0, HEADER_HEIGHT - 2, self.width, 2, COLOR_HEADER_BORDER);

        self.draw_str_at(
            "FipSOs v1.8 (Web Browser + WiFi 802.11 + Audio PCM + SATA + Task Ctrl)",
            TEXT_LEFT_X,
            8,
            COLOR_HEADER_TITLE,
            2,
        );
        self.draw_str_at(
            "Browser fipsos.org | wlan0/eth0 | /dev/snd/pcm0 | sys_unlink | sys_kill",
            TEXT_LEFT_X,
            30,
            COLOR_HEADER_SUBTITLE,
            1,
        );
    }

    fn reset_console_surface(&mut self) {
        if self.fb_addr == 0 {
            return;
        }
        self.fill_rect(0, 0, self.width, self.height, BOOT_FB_COLOR_RGB);
        self.cursor_col = 0;
        self.cursor_row = 0;
        self.fg_color = COLOR_TEXT_DEFAULT;
        self.draw_header_banner();
    }

    fn newline(&mut self) {
        self.cursor_col = 0;
        self.cursor_row += 1;
        self.fg_color = COLOR_TEXT_DEFAULT;

        if self.cursor_row >= self.max_rows && self.max_rows > 0 {
            self.scroll_up_one_line();
            self.cursor_row = self.max_rows - 1;
        }
    }

    fn scroll_up_one_line(&self) {
        if self.fb_addr == 0 || self.max_rows == 0 {
            return;
        }
        if let Some(mut overlay) = MOUSE_OVERLAY.try_lock() {
            if overlay.has_prev {
                for dy in 0..CURSOR_SIZE {
                    let py = overlay.prev_y + dy;
                    if py < self.height {
                        for dx in 0..CURSOR_SIZE {
                            let px = overlay.prev_x + dx;
                            if px < self.width {
                                self.put_pixel(px, py, overlay.saved_patch[dy * CURSOR_SIZE + dx]);
                            }
                        }
                    }
                }
                overlay.has_prev = false;
            }
        }
        let text_area_height = self.max_rows * LINE_HEIGHT;
        let copy_rows = text_area_height.saturating_sub(LINE_HEIGHT);

        for y in 0..copy_rows {
            let dst_y = TEXT_TOP_Y + y;
            let src_y = dst_y + LINE_HEIGHT;
            let dst_base = dst_y * self.pitch_pixels;
            let src_base = src_y * self.pitch_pixels;
            // Preserve the top-right Zero-Copy graphical widget overlay (x >= 1076, y <= 170)
            let row_width = if src_y <= 170 {
                self.width.min(1076)
            } else {
                self.width
            };
            // SAFETY: `dst_base` and `src_base + row_width` are strictly within the mapped framebuffer bounds.
            unsafe {
                let base_ptr = self.fb_addr as *mut u32;
                core::ptr::copy(base_ptr.add(src_base), base_ptr.add(dst_base), row_width);
            }
        }

        let clear_y = TEXT_TOP_Y + copy_rows;
        self.fill_rect(0, clear_y, self.width, LINE_HEIGHT, BOOT_FB_COLOR_RGB);
    }

    fn auto_select_line_color(&mut self, s: &str) {
        let trimmed = s.trim_start_matches('\n');
        if trimmed.starts_with("[ostd") {
            self.fg_color = COLOR_TEXT_OSTD;
        } else if trimmed.starts_with("[FipSOs") || trimmed.starts_with("[MOTD]") {
            self.fg_color = COLOR_TEXT_FIPSOS;
        } else if trimmed.starts_with("[linux_abi") || trimmed.starts_with("[VFS::ls]") {
            self.fg_color = COLOR_TEXT_LINUX_ABI;
        } else if trimmed.starts_with("[Async") {
            self.fg_color = COLOR_TEXT_ASYNC;
        } else if trimmed.starts_with("[Scheduler")
            || trimmed.starts_with("[Thread")
            || trimmed.starts_with("[Mouse")
        {
            self.fg_color = COLOR_TEXT_SCHEDULER;
        } else if trimmed.starts_with("[UserRing3]")
            || trimmed.starts_with("[ZeroCopy")
            || trimmed.starts_with("[Shell")
            || trimmed.starts_with("[npu_infer")
            || trimmed.starts_with("[avatar_cpp")
            || trimmed.starts_with("fipsos@")
        {
            self.fg_color = COLOR_TEXT_RING3;
        }
    }

    pub fn write_text(&mut self, s: &str) {
        if self.fb_addr == 0 || self.max_cols == 0 || self.max_rows == 0 {
            return;
        }
        if self.cursor_col == 0 {
            self.auto_select_line_color(s);
        }

        for b in s.bytes() {
            match b {
                b'\n' => self.newline(),
                b'\r' => self.cursor_col = 0,
                0x08 => {
                    if self.cursor_col > 0 {
                        self.cursor_col -= 1;
                        let px_x = TEXT_LEFT_X + self.cursor_col * CHAR_WIDTH;
                        let px_y = TEXT_TOP_Y + self.cursor_row * LINE_HEIGHT;
                        self.fill_rect(px_x, px_y, CHAR_WIDTH, LINE_HEIGHT, BOOT_FB_COLOR_RGB);
                    }
                }
                ch => {
                    if self.cursor_col >= self.max_cols {
                        let keep_color = self.fg_color;
                        self.newline();
                        self.fg_color = keep_color;
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
            }
        }
    }

    fn present_rect(&self, src_bytes: &[u8], x: usize, y: usize, width: usize, height: usize) {
        if self.fb_addr == 0 || width == 0 || height == 0 {
            return;
        }

        // Draw a 2px neon frame and title around the Zero-Copy widget in the upper-right corner
        let border_x = x.saturating_sub(2);
        let border_y = y.saturating_sub(14);
        self.fill_rect(border_x, border_y, width + 4, height + 16, COLOR_HEADER_BG);
        self.fill_rect(border_x, border_y, width + 4, 1, COLOR_HEADER_BORDER);
        self.fill_rect(
            border_x,
            y + height + 1,
            width + 4,
            1,
            COLOR_HEADER_BORDER,
        );
        self.fill_rect(border_x, border_y, 1, height + 16, COLOR_HEADER_BORDER);
        self.fill_rect(
            x + width + 1,
            border_y,
            1,
            height + 16,
            COLOR_HEADER_BORDER,
        );
        self.draw_str_at(
            "ZERO-COPY /dev/fb0",
            x + 4,
            y.saturating_sub(11),
            COLOR_HEADER_TITLE,
            1,
        );

        for dy in 0..height {
            let py = y + dy;
            if py >= self.height {
                break;
            }
            for dx in 0..width {
                let px = x + dx;
                if px >= self.width {
                    break;
                }
                let byte_idx = (dy * width + dx) * 4;
                if byte_idx + 4 <= src_bytes.len() {
                    let pixel = u32::from_le_bytes([
                        src_bytes[byte_idx],
                        src_bytes[byte_idx + 1],
                        src_bytes[byte_idx + 2],
                        src_bytes[byte_idx + 3],
                    ]);
                    self.put_pixel(px, py, pixel);
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

static FB_CONSOLE: SpinLock<FramebufferConsole> = SpinLock::new(FramebufferConsole::new());

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

/// Lock-free IRQ0 visual heartbeat pulse for `npu-telemetryd` rendered in the top-right header bar.
pub fn draw_scheduler_telemetry_pulse(ticks: u64, preemptions: u64) {
    if DESKTOP_MODE.load(Ordering::Acquire) {
        return;
    }
    let fb_addr = LOCKFREE_FB_ADDR.load(Ordering::Acquire);
    let width = LOCKFREE_FB_WIDTH.load(Ordering::Acquire);
    let height = LOCKFREE_FB_HEIGHT.load(Ordering::Acquire);
    let pitch_pixels = LOCKFREE_FB_PITCH_PIXELS.load(Ordering::Acquire);
    if fb_addr == 0 || width < 1260 || height < 48 || pitch_pixels == 0 {
        return;
    }

    let phase = ((preemptions.wrapping_add(ticks / 10)) & 0x3) as u32;
    let pulse_color = match phase {
        0 => 0x0000_F5D4, // Cyber Cyan
        1 => 0x007A_E582, // Matrix Emerald
        2 => 0x00FF_D166, // Amber Gold
        _ => 0x00C7_7DFF, // Neural Violet
    };

    let x0 = width.saturating_sub(36);
    let y0 = 12usize;
    for dy in 0..20usize {
        let row_base = (y0 + dy) * pitch_pixels;
        for dx in 0..20usize {
            let color = if dy == 0 || dy == 19 || dx == 0 || dx == 19 {
                COLOR_HEADER_BORDER
            } else {
                pulse_color
            };
            // SAFETY: `(x0 + dx, y0 + dy)` is within the top-right header banner of the linear framebuffer.
            unsafe {
                let ptr = (fb_addr as *mut u32).add(row_base + x0 + dx);
                core::ptr::write_volatile(ptr, color);
            }
        }
    }
}

/// Returns the active linear framebuffer dimensions `(width, height)`, or `(1280, 800)` prior to init.
pub fn dimensions() -> (usize, usize) {
    let w = LOCKFREE_FB_WIDTH.load(Ordering::Acquire);
    let h = LOCKFREE_FB_HEIGHT.load(Ordering::Acquire);
    if w >= 320 && h >= 200 {
        (w, h)
    } else {
        (1280, 800)
    }
}

const CURSOR_SIZE: usize = 12;

/// 12x12 Neon Arrow Cursor Sprite (`0` = transparent, `1` = dark outline, `2` = neon core).
static CURSOR_ARROW_12X12: [[u8; CURSOR_SIZE]; CURSOR_SIZE] = [
    [1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [1, 2, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [1, 2, 2, 1, 0, 0, 0, 0, 0, 0, 0, 0],
    [1, 2, 2, 2, 1, 0, 0, 0, 0, 0, 0, 0],
    [1, 2, 2, 2, 2, 1, 0, 0, 0, 0, 0, 0],
    [1, 2, 2, 2, 2, 2, 1, 0, 0, 0, 0, 0],
    [1, 2, 2, 2, 2, 2, 2, 1, 0, 0, 0, 0],
    [1, 2, 2, 2, 2, 1, 1, 1, 1, 0, 0, 0],
    [1, 2, 2, 1, 2, 2, 1, 0, 0, 0, 0, 0],
    [1, 2, 1, 0, 1, 2, 2, 1, 0, 0, 0, 0],
    [1, 1, 0, 0, 0, 1, 2, 1, 0, 0, 0, 0],
    [0, 0, 0, 0, 0, 0, 1, 1, 0, 0, 0, 0],
];

struct MouseCursorOverlay {
    prev_x: usize,
    prev_y: usize,
    has_prev: bool,
    saved_patch: [u32; CURSOR_SIZE * CURSOR_SIZE],
}

impl MouseCursorOverlay {
    const fn new() -> Self {
        Self {
            prev_x: 0,
            prev_y: 0,
            has_prev: false,
            saved_patch: [0; CURSOR_SIZE * CURSOR_SIZE],
        }
    }
}

static MOUSE_OVERLAY: SpinLock<MouseCursorOverlay> = SpinLock::new(MouseCursorOverlay::new());

/// IRQ-safe cursor & header HUD renderer:
/// 1. Restores the saved 12x12 background patch at the previous pointer location (zero trails).
/// 2. Draws a 12x12 neon arrow cursor at `(x, y)` colored according to `buttons`.
/// 3. Renders `Mouse(XXXX,YYYY) B:X` in the top-right FipSOs header bar.
pub fn draw_mouse_cursor_overlay(x: usize, y: usize, buttons: u8) {
    let fb_addr = LOCKFREE_FB_ADDR.load(Ordering::Acquire);
    let width = LOCKFREE_FB_WIDTH.load(Ordering::Acquire);
    let height = LOCKFREE_FB_HEIGHT.load(Ordering::Acquire);
    let pitch_pixels = LOCKFREE_FB_PITCH_PIXELS.load(Ordering::Acquire);
    if fb_addr == 0 || width < 640 || height < 200 || pitch_pixels == 0 {
        return;
    }

    let Some(mut overlay) = MOUSE_OVERLAY.try_lock() else {
        return;
    };

    let base_ptr = fb_addr as *mut u32;

    // 1. Restore previous 12x12 background patch so moving the cursor never leaves trails
    if overlay.has_prev {
        for dy in 0..CURSOR_SIZE {
            let py = overlay.prev_y + dy;
            if py >= height {
                break;
            }
            let row_base = py * pitch_pixels;
            for dx in 0..CURSOR_SIZE {
                let px = overlay.prev_x + dx;
                if px >= width {
                    break;
                }
                let saved = overlay.saved_patch[dy * CURSOR_SIZE + dx];
                // SAFETY: `(px, py)` is bounds-checked within `(width, height)` in HHDM framebuffer.
                unsafe {
                    core::ptr::write_volatile(base_ptr.add(row_base + px), saved);
                }
            }
        }
    }

    let clamped_x = x.min(width.saturating_sub(1));
    let clamped_y = y.min(height.saturating_sub(1));

    // 2. Capture fresh 12x12 background patch under `(clamped_x, clamped_y)` and paint neon arrow
    let fill_color = if (buttons & 0x01) != 0 {
        0x00FF_D166 // Left button: Amber Gold
    } else if (buttons & 0x02) != 0 {
        0x00FF_6B6B // Right button: Coral Neon
    } else if (buttons & 0x04) != 0 {
        0x00C7_7DFF // Middle button: Neural Violet
    } else {
        0x0000_F5D4 // Idle: Cyber Cyan
    };

    for dy in 0..CURSOR_SIZE {
        let py = clamped_y + dy;
        if py >= height {
            break;
        }
        let row_base = py * pitch_pixels;
        for dx in 0..CURSOR_SIZE {
            let px = clamped_x + dx;
            if px >= width {
                break;
            }
            // SAFETY: `(px, py)` is bounds-checked within `(width, height)`.
            unsafe {
                let ptr = base_ptr.add(row_base + px);
                overlay.saved_patch[dy * CURSOR_SIZE + dx] = core::ptr::read_volatile(ptr);
                match CURSOR_ARROW_12X12[dy][dx] {
                    1 => core::ptr::write_volatile(ptr, COLOR_HEADER_BG),
                    2 => core::ptr::write_volatile(ptr, fill_color),
                    _ => {}
                }
            }
        }
    }

    overlay.prev_x = clamped_x;
    overlay.prev_y = clamped_y;
    overlay.has_prev = true;

    // 3. Render `Mouse(XXXX,YYYY) B:X` (20 chars = 160 px) in the top-right FipSOs header bar (TTY mode only)
    if !DESKTOP_MODE.load(Ordering::Acquire) && width >= 1100 && height >= HEADER_HEIGHT {
        let mut hud = [b' '; 20];
        hud[0..6].copy_from_slice(b"Mouse(");
        write_4digits(&mut hud[6..10], clamped_x);
        hud[10] = b',';
        write_4digits(&mut hud[11..15], clamped_y);
        hud[15..19].copy_from_slice(b") B:");
        hud[19] = b'0' + (buttons & 0x07);

        let hud_x0 = width.saturating_sub(212);
        let hud_y0 = 18usize;
        for (i, &ch) in hud.iter().enumerate() {
            let glyph_idx = ((ch.clamp(0x20, 0x7E)) - 0x20) as usize;
            let glyph = &FONT_8X8[glyph_idx];
            let cell_x = hud_x0 + i * 8;
            for (gy, &row_bits) in glyph.iter().enumerate() {
                let row_base = (hud_y0 + gy) * pitch_pixels;
                for gx in 0..8 {
                    let px = cell_x + gx;
                    if px < width {
                        let bit_on = (row_bits & (0x80 >> gx)) != 0;
                        let col = if bit_on { fill_color } else { COLOR_HEADER_BG };
                        // SAFETY: Header bar coordinates `(px, hud_y0 + gy)` are inside `(width, height)`.
                        unsafe {
                            core::ptr::write_volatile(base_ptr.add(row_base + px), col);
                        }
                    }
                }
            }
        }
    }
}

#[inline]
fn write_4digits(dst: &mut [u8], val: usize) {
    let v = val.min(9999);
    dst[0] = b'0' + ((v / 1000) % 10) as u8;
    dst[1] = b'0' + ((v / 100) % 10) as u8;
    dst[2] = b'0' + ((v / 10) % 10) as u8;
    dst[3] = b'0' + (v % 10) as u8;
}

/// Switches `/dev/fb0` between full-screen Ring 3 Desktop Compositor mode (`active == true`)
/// and standard Ring 0 TTY Console mode (`active == false`).
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
pub fn flip_dma_double_buffer(dma_buf: &DmaBuffer, x: usize, y: usize, w: usize, h: usize) -> usize {
    let fb_addr = LOCKFREE_FB_ADDR.load(Ordering::Acquire);
    let width = LOCKFREE_FB_WIDTH.load(Ordering::Acquire);
    let height = LOCKFREE_FB_HEIGHT.load(Ordering::Acquire);
    let pitch_pixels = LOCKFREE_FB_PITCH_PIXELS.load(Ordering::Acquire);
    if fb_addr == 0 || width == 0 || height == 0 || pitch_pixels == 0 || w == 0 || h == 0 {
        return 0;
    }
    if x >= width || y >= height {
        return 0;
    }

    let copy_w = w.min(width - x);
    let copy_h = h.min(height - y);
    let required_bytes = ((y + copy_h - 1) * width + (x + copy_w)) * 4;
    if required_bytes > dma_buf.size_bytes() {
        return 0;
    }

    let mut overlay = MOUSE_OVERLAY.lock();
    let dst_base = fb_addr as *mut u32;
    let src_base = dma_buf.vaddr() as *const u32;

    // 1. Erase previous cursor patch before blitting so moving cursors outside dirty_rect never leave trails
    if overlay.has_prev {
        for dy in 0..CURSOR_SIZE {
            let py = overlay.prev_y + dy;
            if py >= height {
                break;
            }
            let row_base = py * pitch_pixels;
            for dx in 0..CURSOR_SIZE {
                let px = overlay.prev_x + dx;
                if px >= width {
                    break;
                }
                let saved = overlay.saved_patch[dy * CURSOR_SIZE + dx];
                // SAFETY: `(px, py)` is bounds-checked within `(width, height)`.
                unsafe {
                    core::ptr::write_volatile(dst_base.add(row_base + px), saved);
                }
            }
        }
        overlay.has_prev = false;
    }

    // 2. High-bandwidth scanline burst transfer from Zero-Copy Backbuffer VMO to Hardware Frontbuffer
    for row in 0..copy_h {
        let ry = y + row;
        let src_off = ry * width + x;
        let dst_off = ry * pitch_pixels + x;
        // SAFETY: `src_off + copy_w` is within `dma_buf.size_bytes() / 4` in HHDM, and
        // `dst_off + copy_w` is within the Limine 32-bpp linear framebuffer; regions do not overlap.
        unsafe {
            core::ptr::copy_nonoverlapping(src_base.add(src_off), dst_base.add(dst_off), copy_w);
        }
    }

    // 3. Atomically composite the hardware PS/2 cursor on top of the freshly flipped Frontbuffer
    #[cfg(target_arch = "x86_64")]
    {
        let snap = crate::arch::mouse::poll_state();
        let clamped_x = (snap.x as usize).min(width.saturating_sub(1));
        let clamped_y = (snap.y as usize).min(height.saturating_sub(1));
        let fill_color = if (snap.buttons & 0x01) != 0 {
            0x00FF_D166
        } else if (snap.buttons & 0x02) != 0 {
            0x00FF_6B6B
        } else if (snap.buttons & 0x04) != 0 {
            0x00C7_7DFF
        } else {
            0x0000_F5D4
        };

        for dy in 0..CURSOR_SIZE {
            let py = clamped_y + dy;
            if py >= height {
                break;
            }
            let row_base = py * pitch_pixels;
            for dx in 0..CURSOR_SIZE {
                let px = clamped_x + dx;
                if px >= width {
                    break;
                }
                // SAFETY: `(px, py)` is bounds-checked within `(width, height)`.
                unsafe {
                    let ptr = dst_base.add(row_base + px);
                    overlay.saved_patch[dy * CURSOR_SIZE + dx] = core::ptr::read_volatile(ptr);
                    match CURSOR_ARROW_12X12[dy][dx] {
                        1 => core::ptr::write_volatile(ptr, COLOR_HEADER_BG),
                        2 => core::ptr::write_volatile(ptr, fill_color),
                        _ => {}
                    }
                }
            }
        }
        overlay.prev_x = clamped_x;
        overlay.prev_y = clamped_y;
        overlay.has_prev = true;
    }

    copy_w * copy_h
}

/// Blits an RGB32 (`u32`) widget directly from a Zero-Copy `DmaBuffer` without allocating on the Kernel Heap.
pub fn present_dma_widget_rect(dma_buf: &DmaBuffer, x: usize, y: usize, width: usize, height: usize) {
    if DESKTOP_MODE.load(Ordering::Acquire) {
        return;
    }
    let byte_len = width.saturating_mul(height).saturating_mul(4);
    if byte_len == 0 || byte_len > dma_buf.size_bytes() {
        return;
    }
    // SAFETY: `dma_buf.vaddr()` maps `dma_buf.size_bytes()` bytes in HHDM.
    let slice = unsafe { core::slice::from_raw_parts(dma_buf.vaddr() as *const u8, byte_len) };
    FB_CONSOLE.lock().present_rect(slice, x, y, width, height);
}

/// Blits an RGB32 (`u32`) pixel rectangle from a Zero-Copy `Vmo` slice directly onto
/// the hardware linear framebuffer at `(x, y)` without erasing the text console.
pub fn present_vmo_rect(src_bytes: &[u8], x: usize, y: usize, width: usize, height: usize) {
    if DESKTOP_MODE.load(Ordering::Acquire) {
        return;
    }
    FB_CONSOLE
        .lock()
        .present_rect(src_bytes, x, y, width, height);
}

/// Internal formatting entrypoint used by `ostd::fb_print!` and `ostd::fb_println!`.
#[doc(hidden)]
pub fn _print(args: fmt::Arguments<'_>) {
    if DESKTOP_MODE.load(Ordering::Acquire) {
        return;
    }
    let mut guard = FB_CONSOLE.lock();
    let _ = guard.write_fmt(args);
}
