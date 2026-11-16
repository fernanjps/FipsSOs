#pragma once
#include "ui_renderer.hpp"

namespace fipsos::ui {

Rect win_shadow_rect(const FloatingWindow& w) noexcept;
void focus_window(int wid) noexcept;
void open_window_instant(int wid) noexcept;
int64_t exec_child_capturing(const char* prog, const char* const* argv, const char* log_path, bool append, bool detach_stdin = true) noexcept;
void switch_workspace(int ws) noexcept;

void draw_window_chrome(const FloatingWindow& w, bool focused) noexcept;
void draw_desktop_shortcuts() noexcept;
void draw_win_sphere(const FloatingWindow& w, bool focused) noexcept;
void draw_floating_window(int wid, bool focused) noexcept;
void draw_start_menu() noexcept;
void draw_taskbar() noexcept;

void compose_region_to_backbuf(int rx, int ry, int rw, int rh) noexcept;
void compose_all_to_backbuf() noexcept;
void repaint_and_flip(const Rect& r) noexcept;

void wm_handle_left_press(int mx, int my) noexcept;
void wm_handle_pointer_motion(int mx, int my, uint8_t btns) noexcept;

} // namespace fipsos::ui
