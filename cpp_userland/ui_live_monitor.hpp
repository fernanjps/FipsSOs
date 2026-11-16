#pragma once
#include "ui_renderer.hpp"

namespace fipsos::ui {

void refresh_live_kernel_telemetry() noexcept;
void live_monitor_run_ram_probe(uint64_t num_pages = 256ULL) noexcept;
void live_monitor_launch_process() noexcept;
void draw_win_live_monitor(const FloatingWindow& w, bool focused) noexcept;
bool live_monitor_handle_click(const FloatingWindow& w, int mx, int my) noexcept;

} // namespace fipsos::ui
