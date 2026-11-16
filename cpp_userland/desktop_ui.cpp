#include "ui_renderer.hpp"
#include "ui_wm.hpp"
#include "ui_explorer.hpp"
#include "ui_apps.hpp"
#include "ui_live_monitor.hpp"
#include "ui_terminal.hpp"
#include "ui_editor.hpp"
#include "ui_security.hpp"

namespace fipsos::ui {

CompositorState g_ui;

namespace {

int s_esc_state = 0;
uint32_t s_esc_idle = 0;

void handle_key(uint8_t key) noexcept {
    if (s_esc_state == 0) {
        if (key == 0x1BU) {
            s_esc_state = 1;
            s_esc_idle  = 0;
            return;
        }
    } else if (s_esc_state == 1) {
        if (key == '[') {
            s_esc_state = 2;
            return;
        }
        s_esc_state = 0;
        g_ui.should_exit = true;
        return;
    } else if (s_esc_state == 2) {
        s_esc_state = 0;
        const int top_w = g_ui.z_order[CompositorState::kWinCount - 1];
        if (top_w == 5 && g_ui.wins[5].open && !g_ui.wins[5].minimized && g_ui.wins[5].workspace == g_ui.current_ws) {
            editor_handle_arrow(key);
            repaint_and_flip(win_shadow_rect(g_ui.wins[5]));
            return;
        }
        if ((key == 'A' || key == 'B') && top_w == 4 && g_ui.wins[4].open && !g_ui.wins[4].minimized && g_ui.wins[4].workspace == g_ui.current_ws) {
            terminal_scroll(key == 'A' ? -2 : 2);
            repaint_and_flip(win_shadow_rect(g_ui.wins[4]));
            return;
        }
        if ((key == 'A' || key == 'B') && top_w == 2 && g_ui.wins[2].workspace == g_ui.current_ws) {
            explorer_step(key == 'A' ? -1 : 1);
            repaint_and_flip(win_shadow_rect(g_ui.wins[2]));
            return;
        }
        if (g_ui.wins[1].open && !g_ui.wins[1].minimized && g_ui.wins[1].workspace == g_ui.current_ws) {
            if (key == 'A')      g_ui.pitch_rad = clamp_f(g_ui.pitch_rad + 0.15f, -1.3f, 1.3f);
            else if (key == 'B') g_ui.pitch_rad = clamp_f(g_ui.pitch_rad - 0.15f, -1.3f, 1.3f);
            else if (key == 'C') g_ui.yaw_rad   = wrap_pi(g_ui.yaw_rad + 0.15f);
            else if (key == 'D') g_ui.yaw_rad   = wrap_pi(g_ui.yaw_rad - 0.15f);
            recompute_sphere_tile();
            repaint_and_flip(win_shadow_rect(g_ui.wins[1]));
        }
        return;
    }

    if (key >= 0x81U && key <= 0x84U) {
        switch_workspace(static_cast<int>(key - 0x81U));
        return;
    }
    const int top_w = g_ui.z_order[CompositorState::kWinCount - 1];
    if (top_w == 5 && g_ui.wins[5].open && !g_ui.wins[5].minimized && g_ui.wins[5].workspace == g_ui.current_ws && !g_ui.start_open) {
        if (key == 0x13U) {
            editor_save_file();
        } else if (key == 0x12U) {
            editor_save_and_run();
        } else {
            editor_handle_key(key);
        }
        repaint_and_flip(win_shadow_rect(g_ui.wins[5]));
        return;
    }
    if (top_w == 4 && g_ui.wins[4].open && !g_ui.wins[4].minimized && g_ui.wins[4].workspace == g_ui.current_ws && !g_ui.start_open) {
        terminal_handle_key(key);
        if (key == '\n' || key == '\r') {
            compose_all_to_backbuf();
            flip_full();
        } else {
            const FloatingWindow& tw = g_ui.wins[4];
            repaint_and_flip(Rect{tw.x + 12, tw.y + tw.h - 72, tw.w - 24, 34});
        }
        return;
    }

    if (key == 'q' || key == 'Q') {
        g_ui.should_exit = true;
    } else if (key == 'e' || key == 'E') {
        focus_window(2);
        explorer_edit_selected();
        compose_all_to_backbuf();
        flip_full();
    } else if (key == 'n' || key == 'N') {
        editor_new_file(g_ui.exp_dir);
        compose_all_to_backbuf();
        flip_full();
    } else if (key == '\n' || key == '\r' || key == 'x' || key == 'X') {
        focus_window(2);
        explorer_execute_selected();
        compose_all_to_backbuf();
        flip_full();
    } else if (key == 'g' || key == 'G') {
        focus_window(2);
        explorer_scroll(CompositorState::kVisibleDirRows);
        compose_all_to_backbuf();
        flip_full();
    } else if (key == ' ' || key == 0x11U) {
        g_ui.start_open = !g_ui.start_open;
        const int sx = (g_ui.screen_w - 440) / 2;
        const int sy = g_ui.taskbar_y - 350 - 8;
        repaint_and_flip(Rect{sx - 6, sy - 6, 452, 362});
    } else if (key >= '1' && key <= '7') {
        const int wid = static_cast<int>(key - '1');
        open_window_instant(wid);
    } else if (key == 'f' || key == 'F') {
        focus_window(2);
        explorer_cycle_file();
        compose_all_to_backbuf();
        flip_full();
    } else if (key == 'r' || key == 'R') {
        explorer_load_directory(g_ui.exp_dir);
        refresh_hwmon_file();
        refresh_live_kernel_telemetry();
        compose_all_to_backbuf();
        flip_full();
    } else if (key == 't' || key == 'T') {
        g_ui.yaw_rad      = wrap_pi(g_ui.yaw_rad + 0.35f);
        g_ui.palette_mode = (g_ui.palette_mode + 1U) & 3U;
        recompute_sphere_tile();
        repaint_and_flip(win_shadow_rect(g_ui.wins[1]));
    } else if (key == 'p' || key == 'P') {
        action_ping_gateway(true);
        refresh_hwmon_file();
        compose_all_to_backbuf();
        flip_full();
    } else if (key == 'u' || key == 'U') {
        action_run_npu();
        compose_all_to_backbuf();
        flip_full();
    } else if (key == 's' || key == 'S') {
        action_save_disk();
        compose_all_to_backbuf();
        flip_full();
    } else if (key == 'b' || key == 'B') {
        action_spawn_bg_task();
        compose_all_to_backbuf();
        flip_full();
    } else if (key == 'k' || key == 'K') {
        action_kill_task();
        compose_all_to_backbuf();
        flip_full();
    } else if (key == 'a' || key == 'A') {
        action_appcenter_cycle();
        compose_all_to_backbuf();
        flip_full();
    } else if (key == 'c' || key == 'C') {
        action_launch_elf("/bin/c_std_app.elf", "c_std_app.elf");
    }
}

void check_standalone_esc() noexcept {
    if (s_esc_state == 1) {
        s_esc_idle += 1U;
        if (s_esc_idle >= 3U) {
            s_esc_state = 0;
            g_ui.should_exit = true;
        }
    }
}

void query_framebuffer_geometry() noexcept {
    using namespace fipsos;
    fb_info finfo{};
    int64_t vres = sys_ioctl(static_cast<uint64_t>(g_ui.fb_fd), FB_IOCTL_GET_INFO, reinterpret_cast<uint64_t>(&finfo));
    if (vres >= 0 && finfo.width > 0 && finfo.height > 0) {
        g_ui.screen_w = static_cast<int>(finfo.width);
        g_ui.screen_h = static_cast<int>(finfo.height);
        g_ui.fb_pitch = static_cast<int>(finfo.pitch > 0 ? finfo.pitch : finfo.width * 4);
        g_ui.bpp      = static_cast<int>(finfo.bpp > 0 ? finfo.bpp : 32);
        return;
    }
    fb_var_screeninfo vinfo{};
    vres = sys_ioctl(static_cast<uint64_t>(g_ui.fb_fd), FBIOGET_VSCREENINFO, reinterpret_cast<uint64_t>(&vinfo));
    if (vres < 0) {
        vres = sys_ioctl(static_cast<uint64_t>(g_ui.fb_fd), FB_IOCTL_GET_VSCREENINFO, reinterpret_cast<uint64_t>(&vinfo));
    }
    if (vres >= 0 && vinfo.xres > 0 && vinfo.yres > 0) {
        g_ui.screen_w = static_cast<int>(vinfo.xres);
        g_ui.screen_h = static_cast<int>(vinfo.yres);
        g_ui.fb_pitch = static_cast<int>(vinfo.bytes_per_line > 0 ? vinfo.bytes_per_line : vinfo.xres * 4);
        g_ui.bpp      = static_cast<int>(vinfo.bits_per_pixel > 0 ? vinfo.bits_per_pixel : 32);
    } else {
        g_ui.screen_w = 1024;
        g_ui.screen_h = 768;
        g_ui.fb_pitch = 4096;
        g_ui.bpp      = 32;
    }
}

void init_compositor_windows() noexcept {
    const int margin_x = 24;
    const int margin_y = 24;
    const int half_w = (g_ui.screen_w - (margin_x * 2 + 16)) / 2;
    const int half_h = (g_ui.taskbar_y - (margin_y * 2 + 16)) / 2;

    g_ui.current_ws = 0;
    g_ui.wins[0] = FloatingWindow{0, margin_x, margin_y, 656, 420, false, false, 0x00F5D4, "Monitor Kernel en Vivo (SMP | RAM R/W | Procesos)", 0};
    g_ui.wins[1] = FloatingWindow{1, margin_x + half_w + 16, margin_y, half_w, half_h, false, false, 0x818CF8, "Visor 3D Phong — FP32 SSE2", 0};
    g_ui.wins[2] = FloatingWindow{2, margin_x, margin_y + half_h + 16, half_w, half_h, false, false, 0x38BDF8, "Explorador VFS Real (/mnt/fat0)", 0};
    g_ui.wins[3] = FloatingWindow{3, margin_x + half_w + 16, margin_y + half_h + 16, half_w, half_h, false, false, 0xFBBF24, "Centro de Aplicaciones & Red TCP", 0};
    g_ui.wins[4] = FloatingWindow{4, 180, 96, 684, 410, false, false, 0x22C55E, "Terminal FipSOs (TTY Integrada)", 1};
    g_ui.wins[5] = FloatingWindow{5, 160, 82, 610, 386, false, false, 0x2DD4BF, "Editor de Codigo FipSOs (.c / .lua / .txt)", 1};
    g_ui.wins[6] = FloatingWindow{6, 170, 88, 636, 404, false, false, 0xF43F5E, "Centro de Seguridad IA & Procesos", 1};
    g_ui.z_order[0] = 6;
    g_ui.z_order[1] = 5;
    g_ui.z_order[2] = 4;
    g_ui.z_order[3] = 0;
    g_ui.z_order[4] = 1;
    g_ui.z_order[5] = 3;
    g_ui.z_order[6] = 2;

    g_ui.yaw_rad      = 0.42f;
    g_ui.pitch_rad    = -0.24f;
    g_ui.light_dir[0] = -0.42f;
    g_ui.light_dir[1] = -0.48f;
    g_ui.light_dir[2] = 0.77f;
    g_ui.palette_mode = 0U;

    for (int i = 0; i < 24; ++i) {
        g_ui.sparkline[i] = static_cast<uint8_t>(10 + ((i * 7) % 22));
    }
    g_ui.spark_head = 0U;
    g_ui.appcenter_selected = 0;
    for (int i = 0; i < 3; ++i) g_ui.appcenter_installed[i] = false;
    set_cstr(g_ui.appcenter_console, sizeof(g_ui.appcenter_console), "Listo. Servidor http://10.0.2.2:8080/ [TCP]");
    set_cstr(g_ui.appcenter_last_exec, sizeof(g_ui.appcenter_last_exec), "Ninguna");
    set_cstr(g_ui.last_action, sizeof(g_ui.last_action), "Escritorio limpio FipSOs v6.3 (1-7 apps | ESC/Q TTY)");
    g_ui.bat_capacity_pct = 0U;
    g_ui.bat_present      = false;
    g_ui.bat_ac_online    = true;
    g_ui.bat_low          = false;
    set_cstr(g_ui.bat_status, sizeof(g_ui.bat_status), "NotPresent");
    set_cstr(g_ui.bat_profile, sizeof(g_ui.bat_profile), "balanced");
    g_ui.ai_guard_raw[0]  = '\0';
    g_ui.ai_guard_len     = 0;
}

void process_mouse_events() noexcept {
    using namespace fipsos;
    if (g_ui.mouse_fd < 0) return;
    const int64_t mraw = sys_ioctl(static_cast<uint64_t>(g_ui.mouse_fd), MOUSE_IOCTL_POLL_STATE, 0ULL);
    if (mraw < 0) return;

    const MousePacketState ms = unpack_mouse_state(static_cast<uint64_t>(mraw));
    if (ms.event_seq == g_ui.mouse_seq && ms.buttons == g_ui.mouse_btns) return;

    const bool left_down = (ms.buttons & MOUSE_BTN_LEFT) != 0U;
    const bool was_down  = (g_ui.prev_btns & MOUSE_BTN_LEFT) != 0U;
    g_ui.mouse_x    = ms.x;
    g_ui.mouse_y    = ms.y;
    g_ui.mouse_btns = ms.buttons;
    g_ui.mouse_seq  = ms.event_seq;

    if (left_down && !was_down)       wm_handle_left_press(ms.x, ms.y);
    else if (!left_down && was_down)  g_ui.dragging_win = -1;
    else                              wm_handle_pointer_motion(ms.x, ms.y, ms.buttons);
    g_ui.prev_btns = ms.buttons;
}

} // namespace

void open_window_instant(int wid) noexcept {
    if (wid < 0 || wid >= CompositorState::kWinCount) return;
    const bool need_load = (!g_ui.wins[wid].open) || (wid == 2 && g_ui.exp_count == 0);
    g_ui.start_open = false;
    g_ui.wins[wid].workspace = g_ui.current_ws;
    focus_window(wid);
    compose_all_to_backbuf();
    flip_full();
    if (!need_load) return;
    if (wid == 0) {
        refresh_live_kernel_telemetry();
        refresh_ai_guard_cache();
        read_bat0_telemetry(false);
    } else if (wid == 2) {
        explorer_load_directory(g_ui.exp_dir);
    } else if (wid == 4) {
        terminal_flush_keys();
    } else if (wid == 6) {
        security_refresh_state();
        refresh_ai_guard_cache();
    }
    repaint_and_flip(win_shadow_rect(g_ui.wins[wid]));
}

int64_t exec_child_capturing(const char* prog, const char* const* argv, const char* log_path, bool append, bool detach_stdin) noexcept {
    using namespace fipsos;
    g_ui.start_open = false;
    const int64_t saved_in  = detach_stdin ? sys_dup(FD_STDIN) : -1;
    const int64_t saved_out = (log_path != nullptr) ? sys_dup(FD_STDOUT) : -1;
    const int64_t saved_err = (log_path != nullptr) ? sys_dup(FD_STDERR) : -1;
    if (detach_stdin) {
        const int64_t nfd = sys_open("/dev/null", O_RDONLY);
        if (nfd >= 0) {
            sys_dup2(static_cast<uint64_t>(nfd), FD_STDIN);
            sys_close(static_cast<uint64_t>(nfd));
        }
    }
    int64_t log_fd = -1;
    if (log_path != nullptr) {
        const uint64_t flags = O_CREAT | O_RDWR | O_APPEND | (append ? 0ULL : O_TRUNC);
        log_fd = sys_openat(AT_FDCWD, log_path, flags, 0644);
        if (log_fd >= 0) {
            sys_dup2(static_cast<uint64_t>(log_fd), FD_STDOUT);
            sys_dup2(static_cast<uint64_t>(log_fd), FD_STDERR);
            sys_close(static_cast<uint64_t>(log_fd));
        }
    }
    const int64_t rc = (argv != nullptr) ? sys_execve_args(prog, argv) : sys_execve(prog);
    if (saved_in >= 0) {
        sys_dup2(static_cast<uint64_t>(saved_in), FD_STDIN);
        sys_close(static_cast<uint64_t>(saved_in));
    }
    if (saved_out >= 0) {
        sys_dup2(static_cast<uint64_t>(saved_out), FD_STDOUT);
        sys_close(static_cast<uint64_t>(saved_out));
    } else if (log_fd >= 0) {
        sys_close(FD_STDOUT);
    }
    if (saved_err >= 0) {
        sys_dup2(static_cast<uint64_t>(saved_err), FD_STDERR);
        sys_close(static_cast<uint64_t>(saved_err));
    } else if (log_fd >= 0) {
        sys_close(FD_STDERR);
    }
    if (g_ui.fb_fd >= 0) {
        sys_ioctl(static_cast<uint64_t>(g_ui.fb_fd), FB_IOCTL_SET_DESKTOP_MODE, 1ULL);
    }
    return rc;
}

} // namespace fipsos::ui

extern "C" void desktop_main() noexcept {
    using namespace fipsos;
    using namespace fipsos::ui;

    print_literal("[desktop_ui.elf] FipSOs v6.3 Modular Desktop Compositor (C++20 | Containers | FAT32 | /dev/fb0) activo!\n");

    g_ui.pid              = sys_getpid();
    g_ui.fb_fd            = sys_open("/dev/fb0", O_RDWR);
    g_ui.mouse_fd         = sys_open("/dev/input/mouse0", O_RDWR);
    g_ui.net_fd           = sys_open("/dev/net/eth0", O_RDWR);
    g_ui.accel_fd         = sys_open("/dev/accel/accel0", O_RDWR);
    g_ui.model_fd         = sys_open("/models/avatar_v1.bin", O_RDONLY);
    g_ui.dragging_win     = -1;
    g_ui.hover_dock       = -1;
    g_ui.hover_start_tile = -1;
    g_ui.start_open       = false;
    g_ui.should_exit      = false;

    if (g_ui.fb_fd < 0) sys_exit(1);

    query_framebuffer_geometry();
    g_ui.taskbar_h = 48;
    g_ui.taskbar_y = g_ui.screen_h - g_ui.taskbar_h;

    const size_t kBackbufBytes = static_cast<size_t>(g_ui.screen_h) * static_cast<size_t>(g_ui.fb_pitch);
    const int64_t mapped = sys_mmap(0, kBackbufBytes, PROT_READ | PROT_WRITE, MAP_SHARED, static_cast<uint64_t>(g_ui.fb_fd), 0);
    if (mapped <= 0) sys_exit(2);
    g_ui.backbuf = reinterpret_cast<uint32_t*>(mapped);
    g_ui.clip    = Rect{0, 0, g_ui.screen_w, g_ui.screen_h};

    init_compositor_windows();
    set_cstr(g_ui.exp_dir, sizeof(g_ui.exp_dir), "/mnt/fat0");
    terminal_init();
    load_desktop_config();
    init_wallpaper_luts();
    recompute_sphere_tile();

    // Populate real RAM/RTC/SMP live telemetry and cursor coordinates BEFORE rendering the very first frame
    refresh_kernel_telemetry();
    refresh_live_kernel_telemetry();
    read_bat0_telemetry(true);
    refresh_ai_guard_cache();
    if (g_ui.mouse_fd >= 0) {
        const int64_t mraw = sys_ioctl(static_cast<uint64_t>(g_ui.mouse_fd), MOUSE_IOCTL_POLL_STATE, 0ULL);
        if (mraw >= 0) {
            const MousePacketState ms = unpack_mouse_state(static_cast<uint64_t>(mraw));
            g_ui.mouse_x    = ms.x;
            g_ui.mouse_y    = ms.y;
            g_ui.mouse_btns = ms.buttons;
            g_ui.prev_btns  = ms.buttons;
            g_ui.mouse_seq  = ms.event_seq;
        }
    }

    sys_ioctl(static_cast<uint64_t>(g_ui.fb_fd), FB_IOCTL_SET_DESKTOP_MODE, 1ULL);
    compose_all_to_backbuf();
    flip_full();

    uint64_t last_tel_tick  = g_ui.sys_tel.irq0_ticks;
    uint64_t last_live_tick = g_ui.sys_tel.irq0_ticks;

    while (!g_ui.should_exit) {
        const uint32_t seq_before = g_ui.mouse_seq;
        const uint8_t  btn_before = g_ui.mouse_btns;
        process_mouse_events();
        bool had_event = (g_ui.mouse_seq != seq_before || g_ui.mouse_btns != btn_before);

        const int64_t key = sys_ioctl(static_cast<uint64_t>(g_ui.fb_fd), FB_IOCTL_POLL_KEY, 0ULL);
        if (key > 0) {
            had_event = true;
            handle_key(static_cast<uint8_t>(key & 0xFF));
        } else {
            check_standalone_esc();
        }

        if (!had_event && !g_ui.should_exit) {
            const Timespec idle_ts{0, 10000000};
            sys_nanosleep(&idle_ts, nullptr);
            refresh_kernel_telemetry();
            if (g_ui.wins[0].open && !g_ui.wins[0].minimized && g_ui.wins[0].workspace == g_ui.current_ws &&
                g_ui.dragging_win < 0 && g_ui.sys_tel.irq0_ticks >= last_live_tick + 20ULL) {
                last_live_tick = g_ui.sys_tel.irq0_ticks;
                refresh_live_kernel_telemetry();
                repaint_and_flip(win_shadow_rect(g_ui.wins[0]));
            }
            if (g_ui.sys_tel.irq0_ticks >= last_tel_tick + 50ULL) {
                last_tel_tick = g_ui.sys_tel.irq0_ticks;
                read_bat0_telemetry(false);
                push_spark_sample(static_cast<uint8_t>(10U + ((g_ui.sys_tel.daemon_switches * 5U) % 24U)));
                repaint_and_flip(Rect{g_ui.screen_w - 360, g_ui.taskbar_y + 6, 348, 36});
                repaint_and_flip(Rect{g_ui.screen_w - 392, 10, 380, 34});
            }
        }
    }

    sys_ioctl(static_cast<uint64_t>(g_ui.fb_fd), FB_IOCTL_SET_DESKTOP_MODE, 0ULL);
    if (g_ui.model_fd >= 0) sys_close(static_cast<uint64_t>(g_ui.model_fd));
    if (g_ui.accel_fd >= 0) sys_close(static_cast<uint64_t>(g_ui.accel_fd));
    if (g_ui.net_fd >= 0)   sys_close(static_cast<uint64_t>(g_ui.net_fd));
    if (g_ui.mouse_fd >= 0) sys_close(static_cast<uint64_t>(g_ui.mouse_fd));
    sys_close(static_cast<uint64_t>(g_ui.fb_fd));

    print_literal("[desktop_ui.elf] Sesion grafica finalizada limpiamente. Retornando a TTY fipsos@v6.3:/$\n");
    sys_exit(0);
}

extern "C" [[noreturn]] __attribute__((naked, section(".text._start"))) void _start() noexcept {
    asm volatile(
        "andq $-16, %%rsp\n\t"
        "call desktop_main\n\t"
        "xorl %%edi, %%edi\n\t"
        "movl $60, %%eax\n\t"
        "syscall\n\t"
        ::: "memory"
    );
}
