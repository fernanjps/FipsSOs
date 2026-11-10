use super::FB_CONSOLE;
use core::sync::atomic::Ordering;
use crate::sync::SpinLock;
use crate::mm::DmaBuffer;
use super::render::*;
use super::{DESKTOP_MODE, LOCKFREE_FB_ADDR, LOCKFREE_FB_WIDTH, LOCKFREE_FB_HEIGHT, LOCKFREE_FB_PITCH_PIXELS};

pub fn init_ram_backbuffer() {
    // Backbuffer is ready and available
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

pub(crate) const CURSOR_SIZE: usize = 12;

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

pub(crate) struct MouseCursorOverlay {
    pub(crate) prev_x: usize,
    pub(crate) prev_y: usize,
    pub(crate) has_prev: bool,
    pub(crate) saved_patch: [u32; CURSOR_SIZE * CURSOR_SIZE],
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

pub(crate) static MOUSE_OVERLAY: SpinLock<MouseCursorOverlay> = SpinLock::new(MouseCursorOverlay::new());

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
