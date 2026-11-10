use super::render::*;

pub const SCROLLBACK_LINES: usize = 256;
pub const MAX_LINE_COLS: usize = 160;

#[derive(Clone, Copy)]
pub struct ConsoleCell {
    pub ch: u8,
    pub fg: u32,
}

impl ConsoleCell {
    pub const fn empty() -> Self {
        Self {
            ch: b' ',
            fg: COLOR_TEXT_DEFAULT,
        }
    }
}

#[derive(Clone, Copy)]
pub struct ConsoleLine {
    pub cells: [ConsoleCell; MAX_LINE_COLS],
    pub len: usize,
}

impl ConsoleLine {
    pub const fn empty() -> Self {
        Self {
            cells: [ConsoleCell::empty(); MAX_LINE_COLS],
            len: 0,
        }
    }
}

/// Thread-safe linear framebuffer text console state.

impl super::FramebufferConsole {
    pub(crate) fn push_current_line_to_history(&mut self) {
        let slot = self.history_total % SCROLLBACK_LINES;
        self.history[slot] = self.current_line;
        self.history_total += 1;
        self.current_line = ConsoleLine::empty();
    }

    pub fn scroll_history_backward(&mut self, lines: usize) {
        let total = self.history_total + 1;
        let max_scroll = total.saturating_sub(self.max_rows);
        if max_scroll > 0 {
            self.scroll_offset = (self.scroll_offset + lines).min(max_scroll);
            self.redraw_view();
        }
    }

    pub fn scroll_history_forward(&mut self, lines: usize) {
        if self.scroll_offset > 0 {
            self.scroll_offset = self.scroll_offset.saturating_sub(lines);
            self.redraw_view();
        }
    }

    pub fn reset_scroll(&mut self) {
        if self.scroll_offset > 0 {
            self.scroll_offset = 0;
            self.redraw_view();
        }
    }
}
