#pragma once
#include "ui_renderer.hpp"

namespace fipsos::ui {

// VFS Real File Explorer (/mnt/fat0, /disk, /bin, /usr/include)
void explorer_load_directory(const char* path) noexcept;
void explorer_switch_dir_tab(int tab_idx) noexcept;
void explorer_select_entry(int idx) noexcept;
void explorer_cycle_file() noexcept;
void explorer_step(int delta) noexcept;
void explorer_scroll(int delta) noexcept;
void explorer_execute_selected() noexcept;
void explorer_edit_selected() noexcept;

void draw_win_explorer(const FloatingWindow& w, bool focused) noexcept;
bool explorer_handle_window_click(const FloatingWindow& w, int mx, int my) noexcept;

} // namespace fipsos::ui
