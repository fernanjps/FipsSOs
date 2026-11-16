#include "ui_wm.hpp"
#include "ui_explorer.hpp"
#include "ui_apps.hpp"
#include "ui_live_monitor.hpp"
#include "ui_terminal.hpp"
#include "ui_editor.hpp"
#include "ui_security.hpp"

namespace fipsos::ui {

Rect win_shadow_rect(const FloatingWindow& w) noexcept {
    return Rect{w.x - 2, w.y - 2, w.w + 8, w.h + 9};
}

void focus_window(int wid) noexcept {
    if (wid < 0 || wid >= CompositorState::kWinCount) return;
    g_ui.wins[wid].open      = true;
    g_ui.wins[wid].minimized = false;
    g_ui.wins[wid].workspace = g_ui.current_ws;
    int pos = 0;
    for (int i = 0; i < CompositorState::kWinCount; ++i) {
        if (g_ui.z_order[i] == wid) {
            pos = i;
            break;
        }
    }
    for (int i = pos; i < CompositorState::kWinCount - 1; ++i) {
        g_ui.z_order[i] = g_ui.z_order[i + 1];
    }
    g_ui.z_order[CompositorState::kWinCount - 1] = wid;
}

void draw_desktop_shortcuts() noexcept {
    const int pill_w = 372;
    const int pill_x = g_ui.screen_w - pill_w - 16;
    blend_round_rect(pill_x, 12, pill_w, 30, 6, 0x0B1628, 204);
    stroke_round_rect(pill_x, 12, pill_w, 30, 6, 0x26466D);

    const uint32_t used_mib  = g_ui.sys_tel.ram_used_kib / 1024U;
    const uint32_t total_mib = (g_ui.sys_tel.ram_total_kib > 0U) ? (g_ui.sys_tel.ram_total_kib / 1024U) : 504U;
    size_t p = 0;
    p = fmt_str(g_ui.fmt, p, "FipSOs v6.3 | CR3 ");
    p = fmt_hex_u64(g_ui.fmt, p, g_ui.sys_tel.active_cr3);
    p = fmt_str(g_ui.fmt, p, " | ");
    p = fmt_u32(g_ui.fmt, p, used_mib);
    p = fmt_str(g_ui.fmt, p, "/");
    p = fmt_u32(g_ui.fmt, p, total_mib);
    p = fmt_str(g_ui.fmt, p, "M");
    draw_text(g_ui.fmt, pill_x + 14, 23, 0x7AE582);

    struct Shortcut {
        int         y;
        const char* label;
        int         kind;
    };
    constexpr Shortcut kShortcuts[4] = {
        {20,  "Explorador", 2},
        {108, "Monitor HW", 0},
        {196, "Esfera 3D",  1},
        {284, "App Center", 3},
    };

    for (int i = 0; i < 4; ++i) {
        const int sx = 20;
        const int sy = kShortcuts[i].y;
        if (pt_in_rect(g_ui.mouse_x, g_ui.mouse_y, sx, sy, 84, 76)) {
            blend_round_rect(sx, sy, 84, 76, 6, 0x38BDF8, 52);
            stroke_round_rect(sx, sy, 84, 76, 6, 0x38BDF8);
        }
        if (kShortcuts[i].kind == 2)      draw_icon_folder(sx + 32, sy + 14);
        else if (kShortcuts[i].kind == 0) draw_icon_pulse(sx + 32, sy + 14);
        else if (kShortcuts[i].kind == 1) draw_icon_orb3d(sx + 32, sy + 14);
        else                              draw_icon_rust_elf(sx + 32, sy + 14);
        draw_text_centered(kShortcuts[i].label, sx, sy + 46, 84, 16, 0xE2E8F0);
    }
}

void draw_window_chrome(const FloatingWindow& w, bool focused) noexcept {
    draw_shadow(w.x, w.y, w.w, w.h);
    blend_round_rect(w.x, w.y, w.w, w.h, 8, 0x0D1829, 238);

    const uint32_t title_bg = focused ? 0x152742 : 0x101E33;
    fill_round_rect(w.x + 1, w.y + 1, w.w - 2, 32, 7, title_bg);
    fill_rect(w.x + 1, w.y + 25, w.w - 2, 8, title_bg);
    fill_rect(w.x + 1, w.y + 33, w.w - 2, 1, focused ? 0x264B73 : 0x1B314C);

    stroke_round_rect(w.x, w.y, w.w, w.h, 8, focused ? w.accent_rgb : 0x2B4263);

    fill_round_rect(w.x + 12, w.y + 11, 10, 10, 3, w.accent_rgb);
    draw_text(w.title, w.x + 28, w.y + 13, focused ? 0xF8FAFC : 0x94A3B8, w.x + w.w - 68);

    const int min_x = w.x + w.w - 58;
    const int btn_y = w.y + 6;
    const bool hov_min = pt_in_rect(g_ui.mouse_x, g_ui.mouse_y, min_x, btn_y, 24, 20);
    fill_round_rect(min_x, btn_y, 24, 20, 4, hov_min ? 0x284366 : 0x1A2E49);
    fill_rect(min_x + 7, btn_y + 10, 10, 2, 0xE2E8F0);

    const int cls_x = w.x + w.w - 30;
    const bool hov_cls = pt_in_rect(g_ui.mouse_x, g_ui.mouse_y, cls_x, btn_y, 24, 20);
    fill_round_rect(cls_x, btn_y, 24, 20, 4, hov_cls ? 0xDC2626 : 0x2A1E2E);
    draw_char('X', cls_x + 8, btn_y + 6, 0xF8FAFC);
}

void draw_win_sphere(const FloatingWindow& w, bool focused) noexcept {
    draw_window_chrome(w, focused);
    const int sx = w.x + 16;
    const int sy = w.y + 46;
    const int clip_r = w.x + w.w - 14;

    stroke_round_rect(sx - 2, sy - 2, kSphereSize + 4, kSphereSize + 4, 4, 0x818CF8);
    for (int py = 0; py < kSphereSize; ++py) {
        for (int px = 0; px < kSphereSize; ++px) {
            put_px(sx + px, sy + py, g_ui.sphere_tile[py * kSphereSize + px]);
        }
    }

    const int dx = sx + kSphereSize + 14;
    const int dy = sy;
    draw_text("3D Z-Buffer Mesh", dx, dy + 4, 0x818CF8, clip_r);
    draw_text("FP32 SSE2 Mat4x4", dx, dy + 20, 0x00F5D4, clip_r);
    draw_text("12 Tris | Depth Z", dx, dy + 36, 0x38BDF8, clip_r);

    size_t p = 0;
    p = fmt_str(g_ui.fmt, p, "Yaw: ");
    p = fmt_u32(g_ui.fmt, p, static_cast<uint32_t>((g_ui.yaw_rad + kPi) * 57.29f));
    p = fmt_str(g_ui.fmt, p, " deg");
    draw_text(g_ui.fmt, dx, dy + 56, 0xE2E8F0, clip_r);

    p = 0;
    p = fmt_str(g_ui.fmt, p, "Pitch: ");
    p = fmt_u32(g_ui.fmt, p, static_cast<uint32_t>((g_ui.pitch_rad + kPi) * 57.29f));
    p = fmt_str(g_ui.fmt, p, " deg");
    draw_text(g_ui.fmt, dx, dy + 72, 0xE2E8F0, clip_r);

    draw_text("Blinn Spec: ^16", dx, dy + 92, 0xFBBF24, clip_r);
    draw_text("Luz: [Drag Raton]", dx, dy + 108, 0x94A3B8, clip_r);

    const char* pal_name = "Aurora Cyan";
    if ((g_ui.palette_mode & 3U) == 1U)      pal_name = "Emerald Neon";
    else if ((g_ui.palette_mode & 3U) == 2U) pal_name = "Violet Glow";
    else if ((g_ui.palette_mode & 3U) == 3U) pal_name = "Solar Amber";
    p = 0;
    p = fmt_str(g_ui.fmt, p, "Modo: ");
    p = fmt_str(g_ui.fmt, p, pal_name);
    draw_text(g_ui.fmt, dx, dy + 126, 0xC084FC, clip_r);

    const int by = w.y + w.h - 36;
    draw_button(w.x + 14,  by, 160, 26, "Rotar 3D (T)",  0x1E1B4B, 0x818CF8, 0xF8FAFC);
    draw_button(w.x + 185, by, 165, 26, "Cambiar Color", 0x311042, 0xC084FC, 0xF8FAFC);
}

void draw_floating_window(int wid, bool focused) noexcept {
    const FloatingWindow& w = g_ui.wins[wid];
    if (!w.open || w.minimized) return;
    if (wid == 0)      draw_win_live_monitor(w, focused);
    else if (wid == 1) draw_win_sphere(w, focused);
    else if (wid == 2) draw_win_explorer(w, focused);
    else if (wid == 3) draw_win_appcenter(w, focused);
    else if (wid == 4) draw_win_terminal(w, focused);
    else if (wid == 5) draw_win_editor(w, focused);
    else               draw_win_security(w, focused);
}

void draw_start_menu() noexcept {
    if (!g_ui.start_open) return;
    const int sw = 440;
    const int sh = 350;
    const int sx = (g_ui.screen_w - sw) / 2;
    const int sy = g_ui.taskbar_y - sh - 8;

    draw_shadow(sx, sy, sw, sh);
    blend_round_rect(sx, sy, sw, sh, 10, 0x0E1B30, 244);
    stroke_round_rect(sx, sy, sw, sh, 10, 0x38BDF8);

    fill_round_rect(sx + 20, sy + 16, sw - 40, 30, 6, 0x081120);
    stroke_round_rect(sx + 20, sy + 16, sw - 40, 30, 6, 0x28476E);
    draw_text("> Buscar /mnt/fat0, ELF64, hwmon o SIMD...", sx + 32, sy + 27, 0x94A3B8);
    draw_text("Anclado - Lanzador de Aplicaciones FipSOs v6.3", sx + 22, sy + 58, 0xE2E8F0);

    struct AppTile {
        const char* title;
        const char* sub;
        int         icon_kind;
    };
    constexpr AppTile kTiles[8] = {
        {"Monitor HW",   "SMP /sys/hwmon",  0},
        {"Visor 3D",     "Shader FP32",     1},
        {"Explorador",   "/mnt/fat0 VFS",   2},
        {"App Center",   "TCP Software",    3},
        {"Terminal TTY", "Consola Shell",   0},
        {"Editor",       ".c / .lua",       2},
        {"Seguridad IA", "Guard AVX2/Log",  3},
        {"avatar_cpp",   "C++20 ELF64",     1},
    };

    for (int i = 0; i < 8; ++i) {
        const int tx = sx + 14 + (i % 4) * 104;
        const int ty = sy + 78 + (i / 4) * 96;
        const bool hov = pt_in_rect(g_ui.mouse_x, g_ui.mouse_y, tx, ty, 98, 84);
        fill_round_rect(tx, ty, 98, 84, 7, hov ? 0x1D3557 : 0x13233B);
        stroke_round_rect(tx, ty, 98, 84, 7, hov ? 0x00F5D4 : (i == 6 ? 0xF43F5E : 0x243E60));

        if (kTiles[i].icon_kind == 0)      draw_icon_pulse(tx + 39, ty + 12);
        else if (kTiles[i].icon_kind == 1) draw_icon_orb3d(tx + 39, ty + 12);
        else if (kTiles[i].icon_kind == 2) draw_icon_folder(tx + 39, ty + 12);
        else                               draw_icon_rust_elf(tx + 39, ty + 12);

        draw_text_centered(kTiles[i].title, tx, ty + 42, 98, 14, 0xF8FAFC);
        draw_text_centered(kTiles[i].sub,   tx, ty + 60, 98, 12, 0x94A3B8);
    }

    fill_rect(sx + 1, sy + sh - 52, sw - 2, 1, 0x223B5B);
    fill_round_rect(sx + 14, sy + sh - 40, 26, 26, 13, 0x0284C7);
    draw_text("FP", sx + 19, sy + sh - 31, 0xFFFFFF);
    draw_text("v6.3", sx + 45, sy + sh - 31, 0x94A3B8);

    draw_button(sx + 82,       sy + sh - 40, 110, 28, "Probar Fault", 0x4A1520, 0xEF4444, 0xFEE2E2);
    draw_button(sx + 82 + 116, sy + sh - 40, 110, 28, "Salir a TTY",  0x2D1B4E, 0xA855F7, 0xF3E8FF);
    draw_button(sx + 82 + 232, sy + sh - 40, 110, 28, "Apagar ACPI",  0x3F1D2B, 0xF43F5E, 0xFFE4E6);
}

void draw_taskbar() noexcept {
    blend_rect(0, g_ui.taskbar_y, g_ui.screen_w, g_ui.taskbar_h, 0x08101E, 224);
    fill_rect(0, g_ui.taskbar_y, g_ui.screen_w, 1, 0x2E4F7A);

    fill_round_rect(14, g_ui.taskbar_y + 8, 136, 32, 6, 0x101E34);
    stroke_round_rect(14, g_ui.taskbar_y + 8, 136, 32, 6, 0x223B5C);
    fill_round_rect(24, g_ui.taskbar_y + 19, 8, 8, 4, 0x10B981);
    draw_text("FipSOs v6.3", 38, g_ui.taskbar_y + 20, 0xE2E8F0);

    for (int wi = 0; wi < 4; ++wi) {
        const int wx = 158 + wi * 26;
        const int wy = g_ui.taskbar_y + 10;
        const bool cur = (wi == g_ui.current_ws);
        fill_round_rect(wx, wy, 22, 28, 5, cur ? 0x164E63 : 0x101E34);
        stroke_round_rect(wx, wy, 22, 28, 5, cur ? 0x00F5D4 : 0x334155);
        char lbl[2] = {static_cast<char>('1' + wi), '\0'};
        draw_text_centered(lbl, wx, wy, 22, 28, cur ? 0x00F5D4 : 0x94A3B8);
    }

    constexpr int dock_stride = 48;
    const int dock_x0 = (g_ui.screen_w - (8 * dock_stride)) / 2;
    const int top_win = g_ui.z_order[CompositorState::kWinCount - 1];

    for (int i = 0; i < 8; ++i) {
        const int bx = dock_x0 + i * dock_stride;
        const int by = g_ui.taskbar_y + 6;
        const bool hov = pt_in_rect(g_ui.mouse_x, g_ui.mouse_y, bx, by, 40, 36);
        const bool act = (i == 0 && g_ui.start_open) ||
                         (i >= 1 && i <= 7 && g_ui.wins[i - 1].open && g_ui.wins[i - 1].workspace == g_ui.current_ws && !g_ui.wins[i - 1].minimized && top_win == (i - 1));
        if (hov || act) {
            fill_round_rect(bx, by, 40, 36, 6, hov ? 0x20385C : 0x162946);
            stroke_round_rect(bx, by, 40, 36, 6, act ? 0x38BDF8 : 0x2E4F7A);
        }

        if (i == 0)      draw_win11_logo(bx + 20, by + 17, 8, 0x38BDF8, 0x0EA5E9, 0x0284C7, 0x00F5D4);
        else if (i == 1) draw_icon_pulse(bx + 10, by + 7);
        else if (i == 2) draw_icon_orb3d(bx + 10, by + 7);
        else if (i == 3) draw_icon_folder(bx + 10, by + 7);
        else if (i == 4) draw_icon_rust_elf(bx + 10, by + 7);
        else if (i == 5) draw_icon_tty(bx + 10, by + 7);
        else if (i == 6) draw_icon_folder(bx + 10, by + 7);
        else             draw_icon_rust_elf(bx + 10, by + 7);

        if (i >= 1 && i <= 7 && g_ui.wins[i - 1].open && g_ui.wins[i - 1].workspace == g_ui.current_ws) {
            const bool is_focused = (!g_ui.wins[i - 1].minimized && top_win == (i - 1));
            const int pw = is_focused ? 16 : 6;
            fill_rect(bx + (40 - pw) / 2, by + 32, pw, 3, is_focused ? 0x00F5D4 : 0x94A3B8);
        }
    }

    constexpr int tray_w = 344;
    const int tray_x = g_ui.screen_w - tray_w - 14;
    fill_round_rect(tray_x, g_ui.taskbar_y + 8, tray_w, 32, 6, 0x101E34);
    stroke_round_rect(tray_x, g_ui.taskbar_y + 8, tray_w, 32, 6, 0x223B5C);
    fill_round_rect(tray_x + 10, g_ui.taskbar_y + 20, 7, 7, 3, (g_ui.sys_tel.net_link_up != 0U) ? 0x4ADE80 : 0xF87171);

    size_t p = 0;
    p = fmt_str(g_ui.fmt, p, "ETH0 | ");
    p = fmt_u32(g_ui.fmt, p, g_ui.sys_tel.ram_used_kib / 1024U);
    p = fmt_str(g_ui.fmt, p, "M | ");
    draw_text(g_ui.fmt, tray_x + 22, g_ui.taskbar_y + 20, 0xE2E8F0);

    const int bat_x = tray_x + 22 + static_cast<int>(p) * 8;
    size_t bp = 0;
    if (!g_ui.bat_present) {
        bp = fmt_str(g_ui.fmt, bp, "AC (No Bat)");
    } else {
        bp = fmt_str(g_ui.fmt, bp, "BAT ");
        bp = fmt_u32(g_ui.fmt, bp, g_ui.bat_capacity_pct);
        bp = fmt_str(g_ui.fmt, bp, g_ui.bat_ac_online ? "% AC" : "% DC");
    }
    const uint32_t bat_col = g_ui.bat_low ? 0xF87171 : (g_ui.bat_ac_online ? 0x4ADE80 : 0xFBBF24);
    draw_text(g_ui.fmt, bat_x, g_ui.taskbar_y + 20, bat_col);

    const int rtc_x = bat_x + static_cast<int>(bp) * 8;
    p = 0;
    p = fmt_str(g_ui.fmt, p, " | ");
    p = fmt_u32_pad2(g_ui.fmt, p, static_cast<uint32_t>(g_ui.sys_tel.rtc_hour));
    p = fmt_str(g_ui.fmt, p, ":");
    p = fmt_u32_pad2(g_ui.fmt, p, static_cast<uint32_t>(g_ui.sys_tel.rtc_min));
    p = fmt_str(g_ui.fmt, p, ":");
    p = fmt_u32_pad2(g_ui.fmt, p, static_cast<uint32_t>(g_ui.sys_tel.rtc_sec));
    draw_text(g_ui.fmt, rtc_x, g_ui.taskbar_y + 20, 0xE2E8F0);
}

void compose_region_to_backbuf(int rx, int ry, int rw, int rh) noexcept {
    const int x0 = clamp_i(rx, 0, g_ui.screen_w);
    const int y0 = clamp_i(ry, 0, g_ui.screen_h);
    const int x1 = clamp_i(rx + rw, 0, g_ui.screen_w);
    const int y1 = clamp_i(ry + rh, 0, g_ui.screen_h);
    if (x0 >= x1 || y0 >= y1) return;

    g_ui.clip = Rect{x0, y0, x1 - x0, y1 - y0};
    render_wallpaper_clipped();

    if (y0 < g_ui.taskbar_y) {
        draw_desktop_shortcuts();
        const int top_id = g_ui.z_order[CompositorState::kWinCount - 1];
        for (int z = 0; z < CompositorState::kWinCount; ++z) {
            const int wid = g_ui.z_order[z];
            const FloatingWindow& w = g_ui.wins[wid];
            if (w.open && !w.minimized && w.workspace == g_ui.current_ws) {
                const Rect wbounds{w.x - 2, w.y - 2, w.w + 8, w.h + 9};
                if (rects_intersect(g_ui.clip, wbounds)) {
                    draw_floating_window(wid, wid == top_id && !g_ui.start_open);
                }
            }
        }
        if (g_ui.start_open) {
            const int sx = (g_ui.screen_w - 440) / 2;
            const int sy = g_ui.taskbar_y - 350 - 8;
            if (rects_intersect(g_ui.clip, Rect{sx - 4, sy - 4, 450, 360})) {
                draw_start_menu();
            }
        }
    }

    if (y1 > g_ui.taskbar_y) {
        draw_taskbar();
    }
    g_ui.clip = Rect{0, 0, g_ui.screen_w, g_ui.screen_h};
}

void compose_all_to_backbuf() noexcept {
    compose_region_to_backbuf(0, 0, g_ui.screen_w, g_ui.screen_h);
}

void repaint_and_flip(const Rect& r) noexcept {
    compose_region_to_backbuf(r.x, r.y, r.w, r.h);
    flip_dirty_rect(r);
}

void switch_workspace(int ws) noexcept {
    if (ws < 0 || ws > 3 || ws == g_ui.current_ws) return;
    g_ui.current_ws = ws;
    g_ui.start_open = false;
    g_ui.dragging_win = -1;
    for (int z = CompositorState::kWinCount - 1; z >= 0; --z) {
        const int wid = g_ui.z_order[z];
        if (g_ui.wins[wid].open && !g_ui.wins[wid].minimized && g_ui.wins[wid].workspace == ws) {
            focus_window(wid);
            break;
        }
    }
    size_t p = fmt_str(g_ui.last_action, 0, "Escritorio Virtual WS");
    p = fmt_u32(g_ui.last_action, p, static_cast<uint32_t>(ws + 1), sizeof(g_ui.last_action));
    fmt_str_cap(g_ui.last_action, sizeof(g_ui.last_action), p, " activo");
    compose_all_to_backbuf();
    flip_full();
}

void wm_handle_left_press(int mx, int my) noexcept {
    const int dock_x0 = (g_ui.screen_w - 384) / 2;
    if (my >= g_ui.taskbar_y) {
        for (int wi = 0; wi < 4; ++wi) {
            if (pt_in_rect(mx, my, 158 + wi * 26, g_ui.taskbar_y + 10, 22, 28)) {
                switch_workspace(wi);
                return;
            }
        }
        for (int i = 0; i < 8; ++i) {
            if (pt_in_rect(mx, my, dock_x0 + i * 48, g_ui.taskbar_y + 6, 40, 36)) {
                if (i == 0) {
                    g_ui.start_open = !g_ui.start_open;
                    const int sx = (g_ui.screen_w - 440) / 2;
                    const int sy = g_ui.taskbar_y - 350 - 8;
                    repaint_and_flip(Rect{sx - 6, sy - 6, 452, 362});
                } else if (i >= 1 && i <= 7) {
                    const int wid = i - 1;
                    g_ui.start_open = false;
                    if (g_ui.wins[wid].open && g_ui.wins[wid].workspace == g_ui.current_ws &&
                        !g_ui.wins[wid].minimized && g_ui.z_order[CompositorState::kWinCount - 1] == wid) {
                        g_ui.wins[wid].minimized = true;
                        compose_all_to_backbuf();
                        flip_full();
                    } else {
                        open_window_instant(wid);
                    }
                }
                return;
            }
        }
        return;
    }

    if (g_ui.start_open) {
        const int sx = (g_ui.screen_w - 440) / 2;
        const int sy = g_ui.taskbar_y - 350 - 8;
        if (pt_in_rect(mx, my, sx, sy, 440, 350)) {
            if (pt_in_rect(mx, my, sx + 82, sy + 310, 110, 28)) {
                action_launch_elf("/bin/fault_test.elf", "fault_test.elf");
                return;
            }
            if (pt_in_rect(mx, my, sx + 198, sy + 310, 110, 28)) {
                g_ui.should_exit = true;
                return;
            }
            if (pt_in_rect(mx, my, sx + 314, sy + 310, 110, 28)) {
                fipsos::sys_reboot(0, 0, 0, 0);
                return;
            }
            for (int i = 0; i < 8; ++i) {
                const int tx = sx + 14 + (i % 4) * 104;
                const int ty = sy + 78 + (i / 4) * 96;
                if (pt_in_rect(mx, my, tx, ty, 98, 84)) {
                    if (i <= 6) {
                        open_window_instant(i);
                    } else {
                        action_launch_elf("/bin/avatar_cpp.elf", "avatar_cpp.elf");
                    }
                    return;
                }
            }
            return;
        }
        g_ui.start_open = false;
        repaint_and_flip(Rect{sx - 6, sy - 6, 452, 362});
    }

    for (int z = CompositorState::kWinCount - 1; z >= 0; --z) {
        const int wid = g_ui.z_order[z];
        FloatingWindow& w = g_ui.wins[wid];
        if (!w.open || w.minimized || w.workspace != g_ui.current_ws || !pt_in_rect(mx, my, w.x, w.y, w.w, w.h)) continue;

        const bool z_changed = (z != CompositorState::kWinCount - 1);
        focus_window(wid);
        if (z_changed && wid == 4) terminal_flush_keys();

        if (pt_in_rect(mx, my, w.x + w.w - 58, w.y + 6, 24, 20)) {
            w.minimized = true;
            compose_all_to_backbuf();
            flip_full();
            return;
        }
        if (pt_in_rect(mx, my, w.x + w.w - 30, w.y + 6, 24, 20)) {
            w.open = false;
            compose_all_to_backbuf();
            flip_full();
            return;
        }
        if (my < w.y + 33) {
            g_ui.dragging_win = wid;
            g_ui.drag_off_x   = mx - w.x;
            g_ui.drag_off_y   = my - w.y;
            if (z_changed) {
                compose_all_to_backbuf();
                flip_full();
            }
            return;
        }

        if (wid == 1) {
            const int by = w.y + w.h - 36;
            if (pt_in_rect(mx, my, w.x + 14, by, 160, 26)) {
                g_ui.yaw_rad = wrap_pi(g_ui.yaw_rad + 0.35f);
                recompute_sphere_tile();
                repaint_and_flip(win_shadow_rect(w));
                return;
            }
            if (pt_in_rect(mx, my, w.x + 185, by, 165, 26)) {
                g_ui.palette_mode = (g_ui.palette_mode + 1U) & 3U;
                recompute_sphere_tile();
                repaint_and_flip(win_shadow_rect(w));
                return;
            }
        } else if (wid == 2) {
            explorer_handle_window_click(w, mx, my);
        } else if (wid == 4) {
            terminal_handle_window_click(w, mx, my);
        } else if (wid == 5) {
            editor_handle_window_click(w, mx, my);
        } else if (wid == 6) {
            security_handle_window_click(w, mx, my);
        } else if (wid == 0) {
            live_monitor_handle_click(w, mx, my);
        } else {
            apps_handle_window_click(wid, w, mx, my);
        }
        compose_all_to_backbuf();
        flip_full();
        return;
    }

    if (mx >= 20 && mx < 104 && my >= 20 && my < 360) {
        if (pt_in_rect(mx, my, 20, 20, 84, 76))       open_window_instant(2);
        else if (pt_in_rect(mx, my, 20, 108, 84, 76)) open_window_instant(0);
        else if (pt_in_rect(mx, my, 20, 196, 84, 76)) open_window_instant(1);
        else if (pt_in_rect(mx, my, 20, 284, 84, 76)) open_window_instant(3);
    }
}

void wm_handle_pointer_motion(int mx, int my, uint8_t btns) noexcept {
    if ((btns & fipsos::MOUSE_BTN_LEFT) != 0U) {
        if (g_ui.dragging_win >= 0) {
            FloatingWindow& w = g_ui.wins[g_ui.dragging_win];
            w.x = clamp_i(mx - g_ui.drag_off_x, 8, g_ui.screen_w - w.w - 8);
            w.y = clamp_i(my - g_ui.drag_off_y, 8, g_ui.taskbar_y - w.h - 6);
            compose_all_to_backbuf();
            flip_full();
            return;
        }
        const FloatingWindow& w1 = g_ui.wins[1];
        if (w1.open && !w1.minimized && w1.workspace == g_ui.current_ws && g_ui.z_order[CompositorState::kWinCount - 1] == 1) {
            const int vx = w1.x + 16;
            const int vy = w1.y + 46;
            if (pt_in_rect(mx, my, vx, vy, kSphereSize, kSphereSize)) {
                const float nx = (static_cast<float>(mx - vx) - 66.0f) * (1.0f / 66.0f);
                const float ny = (static_cast<float>(my - vy) - 66.0f) * (1.0f / 66.0f);
                const float lz = 0.72f;
                const float inv = 1.0f / sqrtf_sse2(nx * nx + ny * ny + lz * lz);
                g_ui.light_dir[0] = nx * inv;
                g_ui.light_dir[1] = ny * inv;
                g_ui.light_dir[2] = lz * inv;
                g_ui.yaw_rad      = wrap_pi(nx * 1.5f);
                g_ui.pitch_rad    = wrap_pi(ny * 1.1f);
                recompute_sphere_tile();
                repaint_and_flip(Rect{vx - 2, vy - 2, kSphereSize + 4, kSphereSize + 4});
                return;
            }
        }
    }

    const int dock_x0 = (g_ui.screen_w - 384) / 2;
    int new_dock = -1;
    if (my >= g_ui.taskbar_y) {
        for (int i = 0; i < 8; ++i) {
            if (pt_in_rect(mx, my, dock_x0 + i * 48, g_ui.taskbar_y + 6, 40, 36)) {
                new_dock = i;
                break;
            }
        }
    }
    if (new_dock != g_ui.hover_dock) {
        g_ui.hover_dock = new_dock;
        repaint_and_flip(Rect{dock_x0 - 4, g_ui.taskbar_y, 388, g_ui.taskbar_h});
    }

    if (g_ui.start_open) {
        const int sx = (g_ui.screen_w - 440) / 2;
        const int sy = g_ui.taskbar_y - 350 - 8;
        int new_tile = -1;
        for (int i = 0; i < 8; ++i) {
            const int tx = sx + 14 + (i % 4) * 104;
            const int ty = sy + 78 + (i / 4) * 96;
            if (pt_in_rect(mx, my, tx, ty, 98, 84)) {
                new_tile = i;
                break;
            }
        }
        if (new_tile != g_ui.hover_start_tile) {
            g_ui.hover_start_tile = new_tile;
            repaint_and_flip(Rect{sx, sy, 440, 350});
        }
    }
}

} // namespace fipsos::ui
