#pragma once
#include "ui_renderer.hpp"

namespace fipsos::ui {

void editor_open_file(const char* path, const char* display_name) noexcept;
void editor_new_file(const char* dir) noexcept;
void editor_save_file() noexcept;
void editor_save_and_run() noexcept;
void editor_scroll(int delta) noexcept;
void editor_handle_key(uint8_t key) noexcept;
void editor_handle_arrow(uint8_t code) noexcept;

void draw_win_editor(const FloatingWindow& w, bool focused) noexcept;
bool editor_handle_window_click(const FloatingWindow& w, int mx, int my) noexcept;

} // namespace fipsos::ui
