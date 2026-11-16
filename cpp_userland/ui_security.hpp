#pragma once
#include "ui_renderer.hpp"

namespace fipsos::ui {

void security_refresh_state() noexcept;
void draw_win_security(const FloatingWindow& w, bool focused) noexcept;
bool security_handle_window_click(const FloatingWindow& w, int mx, int my) noexcept;

} // namespace fipsos::ui
