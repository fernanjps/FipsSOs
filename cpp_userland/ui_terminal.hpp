#pragma once
#include "ui_renderer.hpp"

namespace fipsos::ui {

void terminal_init() noexcept;
void terminal_flush_keys() noexcept;
void terminal_append_line(const char* line) noexcept;
void terminal_exec_command(const char* cmdline) noexcept;
void terminal_handle_key(uint8_t key) noexcept;
void terminal_scroll(int delta) noexcept;

void draw_win_terminal(const FloatingWindow& w, bool focused) noexcept;
bool terminal_handle_window_click(const FloatingWindow& w, int mx, int my) noexcept;

} // namespace fipsos::ui
