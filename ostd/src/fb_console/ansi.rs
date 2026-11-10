use super::FramebufferConsole;
use super::render::*;
use super::scrollback::*;
use crate::boot::BOOT_FB_COLOR_RGB;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnsiState {
    Normal,
    Esc,
    Csi,
}


impl FramebufferConsole {
    pub(crate) fn auto_select_line_color(&mut self, s: &str) {
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

    pub(crate) fn execute_csi_command(&mut self, cmd: u8) {
        match cmd {
            b'm' => self.apply_sgr(),
            b'K' => {
                let mode = self.parse_first_param_default(0);
                match mode {
                    0 => {
                        for c in self.cursor_col..MAX_LINE_COLS {
                            self.current_line.cells[c] = ConsoleCell::empty();
                        }
                        if self.cursor_col < self.current_line.len {
                            self.current_line.len = self.cursor_col;
                        }
                        let px_x = TEXT_LEFT_X + self.cursor_col * CHAR_WIDTH;
                        let px_y = TEXT_TOP_Y + self.cursor_row * LINE_HEIGHT;
                        let clear_w = (self.max_cols.saturating_sub(self.cursor_col)) * CHAR_WIDTH;
                        if clear_w > 0 {
                            self.fill_rect(px_x, px_y, clear_w, LINE_HEIGHT, BOOT_FB_COLOR_RGB);
                        }
                    }
                    1 => {
                        let end = self.cursor_col.min(MAX_LINE_COLS);
                        for c in 0..end {
                            self.current_line.cells[c] = ConsoleCell::empty();
                        }
                        let px_x = TEXT_LEFT_X;
                        let px_y = TEXT_TOP_Y + self.cursor_row * LINE_HEIGHT;
                        let clear_w = end * CHAR_WIDTH;
                        if clear_w > 0 {
                            self.fill_rect(px_x, px_y, clear_w, LINE_HEIGHT, BOOT_FB_COLOR_RGB);
                        }
                    }
                    2 => {
                        for c in 0..MAX_LINE_COLS {
                            self.current_line.cells[c] = ConsoleCell::empty();
                        }
                        self.current_line.len = 0;
                        let px_x = TEXT_LEFT_X;
                        let px_y = TEXT_TOP_Y + self.cursor_row * LINE_HEIGHT;
                        let clear_w = self.max_cols * CHAR_WIDTH;
                        if clear_w > 0 {
                            self.fill_rect(px_x, px_y, clear_w, LINE_HEIGHT, BOOT_FB_COLOR_RGB);
                        }
                    }
                    _ => {}
                }
            }
            b'C' => {
                let count = self.parse_first_param_default(1).max(1) as usize;
                self.cursor_col = (self.cursor_col + count).min(self.max_cols.saturating_sub(1));
            }
            b'D' => {
                let count = self.parse_first_param_default(1).max(1) as usize;
                self.cursor_col = self.cursor_col.saturating_sub(count);
            }
            b'J' => {
                let mode = self.parse_first_param_default(0);
                if mode == 2 {
                    self.cursor_col = 0;
                    self.cursor_row = 0;
                    self.current_line = ConsoleLine::empty();
                    let text_h = self.max_rows * LINE_HEIGHT;
                    self.fill_rect(0, TEXT_TOP_Y, self.width, text_h, BOOT_FB_COLOR_RGB);
                }
            }
            b'H' | b'f' => {
                self.cursor_col = 0;
                self.cursor_row = 0;
            }
            _ => {}
        }
    }

    pub(crate) fn parse_first_param_default(&self, default: u32) -> u32 {
        if self.ansi_len == 0 {
            return default;
        }
        let mut val = 0u32;
        let mut has_digit = false;
        for &b in &self.ansi_buf[..self.ansi_len] {
            if (b'0'..=b'9').contains(&b) {
                val = val.saturating_mul(10).saturating_add((b - b'0') as u32);
                has_digit = true;
            } else if b == b';' {
                break;
            }
        }
        if has_digit {
            val
        } else {
            default
        }
    }

    pub(crate) fn apply_sgr(&mut self) {
        if self.ansi_len == 0 {
            self.fg_color = COLOR_TEXT_DEFAULT;
            return;
        }
        let buf = self.ansi_buf;
        let len = self.ansi_len;
        let mut val = 0u32;
        let mut has_val = false;
        for &b in &buf[..len] {
            if (b'0'..=b'9').contains(&b) {
                val = val.saturating_mul(10).saturating_add((b - b'0') as u32);
                has_val = true;
            } else if b == b';' {
                if has_val {
                    self.apply_sgr_code(val);
                    val = 0;
                    has_val = false;
                }
            }
        }
        if has_val {
            self.apply_sgr_code(val);
        }
    }

    pub(crate) fn apply_sgr_code(&mut self, code: u32) {
        match code {
            0 => self.fg_color = COLOR_TEXT_DEFAULT,
            1 => {
                if self.fg_color == COLOR_TEXT_DEFAULT {
                    self.fg_color = 0x00FF_FFFF;
                }
            }
            30 => self.fg_color = 0x0033_3333,
            31 => self.fg_color = 0x00FF_5555,
            32 => self.fg_color = 0x007A_E582,
            33 => self.fg_color = 0x00FF_D166,
            34 => self.fg_color = 0x005C_E1E6,
            35 => self.fg_color = 0x00C7_7DFF,
            36 => self.fg_color = 0x0000_F5D4,
            37 => self.fg_color = 0x00E6_F1FF,
            39 => self.fg_color = COLOR_TEXT_DEFAULT,
            90 => self.fg_color = 0x0088_8888,
            91 => self.fg_color = 0x00FF_7777,
            92 => self.fg_color = 0x0055_FF55,
            93 => self.fg_color = 0x00FF_E066,
            94 => self.fg_color = 0x0070_C1FF,
            95 => self.fg_color = 0x00D8_96FF,
            96 => self.fg_color = 0x0070_FFFF,
            97 => self.fg_color = 0x00FF_FFFF,
            _ => {}
        }
    }


}
