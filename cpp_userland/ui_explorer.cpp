#include "ui_explorer.hpp"
#include "ui_wm.hpp"
#include "ui_apps.hpp"
#include "ui_editor.hpp"
#include "ui_img_preview.hpp"

namespace fipsos::ui {

namespace {

constexpr const char* kDirTabs[4] = {
    "/mnt/fat0",
    "/disk",
    "/bin",
    "/usr/include",
};

bool has_suffix(const char* name, const char* suf) noexcept {
    const size_t n = str_len(name);
    const size_t m = str_len(suf);
    return (n >= m) && str_eq(name + n - m, suf);
}

bool has_text_ext(const char* name) noexcept {
    return has_suffix(name, ".txt") || has_suffix(name, ".lua")
        || has_suffix(name, ".c")   || has_suffix(name, ".h");
}

void write_stdout_str(const char* s) noexcept {
    fipsos::sys_write(fipsos::FD_STDOUT, s, str_len(s));
}

void sanitize_copy_line(char* dst, size_t cap, const char* src, size_t len) noexcept {
    size_t j = 0;
    for (size_t i = 0; i < len && j + 1 < cap; ++i) {
        const uint8_t c = static_cast<uint8_t>(src[i]);
        if (c == '\t') {
            dst[j++] = ' ';
        } else if (c >= 0x20U && c <= 0x7EU) {
            dst[j++] = static_cast<char>(c);
        } else {
            dst[j++] = '.';
        }
    }
    dst[j] = '\0';
}

void preview_vfs_file(const char* full_path, const char* display_name) noexcept {
    set_cstr(g_ui.exp_preview_title, sizeof(g_ui.exp_preview_title), display_name);
    g_ui.exp_preview_line_count = 0;
    clear_image_preview();
    if (try_load_image_preview(full_path, display_name)) {
        size_t p = fmt_str_cap(g_ui.exp_preview_title, sizeof(g_ui.exp_preview_title), 0, display_name);
        p = fmt_str_cap(g_ui.exp_preview_title, sizeof(g_ui.exp_preview_title), p, " [");
        p = fmt_u32(g_ui.exp_preview_title, p, static_cast<uint32_t>(g_img_prev.w), sizeof(g_ui.exp_preview_title));
        p = fmt_str_cap(g_ui.exp_preview_title, sizeof(g_ui.exp_preview_title), p, "x");
        p = fmt_u32(g_ui.exp_preview_title, p, static_cast<uint32_t>(g_img_prev.h), sizeof(g_ui.exp_preview_title));
        fmt_str_cap(g_ui.exp_preview_title, sizeof(g_ui.exp_preview_title), p, "]");
        return;
    }

    const int64_t fd = fipsos::sys_openat(fipsos::AT_FDCWD, full_path, fipsos::O_RDONLY, 0);
    if (fd < 0) {
        set_cstr(g_ui.exp_preview_lines[0], sizeof(g_ui.exp_preview_lines[0]), "(No se pudo abrir archivo)");
        g_ui.exp_preview_line_count = 1;
        return;
    }

    char raw[512];
    const int64_t nread = fipsos::sys_read(static_cast<uint64_t>(fd), raw, sizeof(raw) - 1);
    fipsos::sys_close(static_cast<uint64_t>(fd));

    if (nread <= 0) {
        set_cstr(g_ui.exp_preview_lines[0], sizeof(g_ui.exp_preview_lines[0]), "(Archivo vacio - 0 bytes)");
        g_ui.exp_preview_line_count = 1;
        return;
    }

    const size_t total = static_cast<size_t>(nread);
    raw[total] = '\0';

    size_t start = 0;
    while (start < total && g_ui.exp_preview_line_count < CompositorState::kMaxPreviewLines) {
        size_t end = start;
        while (end < total && raw[end] != '\n' && raw[end] != '\r') {
            ++end;
        }
        const size_t line_len = end - start;
        const int idx = g_ui.exp_preview_line_count++;
        sanitize_copy_line(g_ui.exp_preview_lines[idx], sizeof(g_ui.exp_preview_lines[idx]), raw + start, line_len);
        start = end;
        if (start < total && raw[start] == '\r') ++start;
        if (start < total && raw[start] == '\n') ++start;
    }

    char log_buf[160];
    size_t lp = 0;
    lp = fmt_str_cap(log_buf, sizeof(log_buf), lp, "[ui_explorer::preview] ");
    lp = fmt_str_cap(log_buf, sizeof(log_buf), lp, full_path);
    lp = fmt_str_cap(log_buf, sizeof(log_buf), lp, " -> ");
    lp = fmt_str_cap(log_buf, sizeof(log_buf), lp, g_ui.exp_preview_lines[0]);
    lp = fmt_str_cap(log_buf, sizeof(log_buf), lp, "\n");
    fipsos::sys_write(fipsos::FD_STDOUT, log_buf, lp);
}

void build_child_path(char* dst, size_t cap, const char* dir, const char* name) noexcept {
    size_t p = 0;
    p = fmt_str_cap(dst, cap, p, dir);
    if (p > 1 && dst[p - 1] != '/') {
        p = fmt_str_cap(dst, cap, p, "/");
    }
    fmt_str_cap(dst, cap, p, name);
}

void log_exec_preview(const char* kind, const char* full_path) noexcept {
    char log_buf[192];
    size_t lp = 0;
    lp = fmt_str_cap(log_buf, sizeof(log_buf), lp, "[ui_explorer::exec] ");
    lp = fmt_str_cap(log_buf, sizeof(log_buf), lp, kind);
    lp = fmt_str_cap(log_buf, sizeof(log_buf), lp, " ");
    lp = fmt_str_cap(log_buf, sizeof(log_buf), lp, full_path);
    lp = fmt_str_cap(log_buf, sizeof(log_buf), lp, " -> ");
    lp = fmt_str_cap(log_buf, sizeof(log_buf), lp,
                     (g_ui.exp_preview_line_count > 0) ? g_ui.exp_preview_lines[0] : "(sin salida)");
    lp = fmt_str_cap(log_buf, sizeof(log_buf), lp, "\n");
    fipsos::sys_write(fipsos::FD_STDOUT, log_buf, lp);
}

} // namespace

void explorer_load_directory(const char* path) noexcept {
    set_cstr(g_ui.exp_dir, sizeof(g_ui.exp_dir), path);
    g_ui.exp_active_tab = -1;
    for (int t = 0; t < 4; ++t) {
        if (str_eq(g_ui.exp_dir, kDirTabs[t])) {
            g_ui.exp_active_tab = t;
            break;
        }
    }

    g_ui.exp_count = 0;
    g_ui.exp_selected = -1;
    g_ui.exp_scroll = 0;
    g_ui.exp_preview_line_count = 0;
    set_cstr(g_ui.exp_preview_title, sizeof(g_ui.exp_preview_title), "(Sin archivo seleccionado)");

    const int64_t dfd = fipsos::sys_openat(fipsos::AT_FDCWD, g_ui.exp_dir, fipsos::O_RDONLY, 0);
    if (dfd < 0) {
        set_cstr(g_ui.exp_status, sizeof(g_ui.exp_status), "No montado / Directorio no encontrado");
        return;
    }

    uint8_t dbuf[1024];
    while (g_ui.exp_count < CompositorState::kMaxDirEntries) {
        const int64_t nbytes = fipsos::sys_getdents64(static_cast<uint64_t>(dfd), dbuf, sizeof(dbuf));
        if (nbytes <= 0) break;

        size_t pos = 0;
        while (pos + 20 <= static_cast<size_t>(nbytes) && g_ui.exp_count < CompositorState::kMaxDirEntries) {
            const uint16_t reclen = static_cast<uint16_t>(dbuf[pos + 16]) |
                                    (static_cast<uint16_t>(dbuf[pos + 17]) << 8);
            if (reclen < 20 || pos + reclen > static_cast<size_t>(nbytes)) break;

            const uint8_t d_type = dbuf[pos + 18];
            const char* d_name   = reinterpret_cast<const char*>(dbuf + pos + 19);
            if (!str_eq(d_name, ".") && !str_eq(d_name, "..")) {
                VfsEntry& e = g_ui.exp_entries[g_ui.exp_count++];
                set_cstr(e.name, sizeof(e.name), d_name);
                e.type = d_type;
                e.size = 0;
            }
            pos += reclen;
        }
    }
    fipsos::sys_close(static_cast<uint64_t>(dfd));

    size_t sp = 0;
    sp = fmt_str_cap(g_ui.exp_status, sizeof(g_ui.exp_status), sp, g_ui.exp_dir);
    sp = fmt_str_cap(g_ui.exp_status, sizeof(g_ui.exp_status), sp, " [");
    char nbuf[16];
    fmt_u32(nbuf, 0, static_cast<uint32_t>(g_ui.exp_count), sizeof(nbuf));
    sp = fmt_str_cap(g_ui.exp_status, sizeof(g_ui.exp_status), sp, nbuf);
    fmt_str_cap(g_ui.exp_status, sizeof(g_ui.exp_status), sp, " entradas reales]");

    write_stdout_str("[ui_explorer] dir=");
    write_stdout_str(g_ui.exp_dir);
    write_stdout_str(" entries=");
    write_stdout_str(nbuf);
    for (int i = 0; i < g_ui.exp_count; ++i) {
        write_stdout_str(i == 0 ? ": " : ", ");
        write_stdout_str(g_ui.exp_entries[i].name);
    }
    write_stdout_str("\n");

    int first_reg = -1;
    int first_txt = -1;
    for (int i = 0; i < g_ui.exp_count; ++i) {
        if (g_ui.exp_entries[i].type == fipsos::DT_REG) {
            if (first_reg < 0) first_reg = i;
            if (first_txt < 0 && has_text_ext(g_ui.exp_entries[i].name)) first_txt = i;
        }
    }
    if (first_txt >= 0) explorer_select_entry(first_txt);
    else if (first_reg >= 0) explorer_select_entry(first_reg);
}

void explorer_switch_dir_tab(int tab_idx) noexcept {
    if (tab_idx < 0 || tab_idx >= 4) return;
    explorer_load_directory(kDirTabs[tab_idx]);
}

void explorer_select_entry(int idx) noexcept {
    if (idx < 0 || idx >= g_ui.exp_count) return;
    const VfsEntry& e = g_ui.exp_entries[idx];
    char full_path[128];
    build_child_path(full_path, sizeof(full_path), g_ui.exp_dir, e.name);

    if (e.type == fipsos::DT_DIR) {
        explorer_load_directory(full_path);
        return;
    }

    g_ui.exp_selected = idx;
    if (idx < g_ui.exp_scroll) {
        g_ui.exp_scroll = idx;
    } else if (idx >= g_ui.exp_scroll + CompositorState::kVisibleDirRows) {
        g_ui.exp_scroll = idx - (CompositorState::kVisibleDirRows - 1);
    }

    if (e.type == fipsos::DT_CHR) {
        clear_image_preview();
        set_cstr(g_ui.exp_preview_title, sizeof(g_ui.exp_preview_title), e.name);
        set_cstr(g_ui.exp_preview_lines[0], sizeof(g_ui.exp_preview_lines[0]), "[Dispositivo de Caracter / VMO Kernel]");
        g_ui.exp_preview_line_count = 1;
        return;
    }
    preview_vfs_file(full_path, e.name);
}

void explorer_step(int delta) noexcept {
    if (g_ui.exp_count <= 0) return;
    int next = g_ui.exp_selected;
    if (next < 0) next = 0;
    else next = (next + delta + g_ui.exp_count) % g_ui.exp_count;

    if (next < g_ui.exp_scroll) {
        g_ui.exp_scroll = next;
    } else if (next >= g_ui.exp_scroll + CompositorState::kVisibleDirRows) {
        g_ui.exp_scroll = next - (CompositorState::kVisibleDirRows - 1);
    }

    if (g_ui.exp_entries[next].type == fipsos::DT_DIR) {
        clear_image_preview();
        g_ui.exp_selected = next;
        set_cstr(g_ui.exp_preview_title, sizeof(g_ui.exp_preview_title), g_ui.exp_entries[next].name);
        set_cstr(g_ui.exp_preview_lines[0], sizeof(g_ui.exp_preview_lines[0]), "[Directorio VFS - Clic/Enter para abrir]");
        g_ui.exp_preview_line_count = 1;
        return;
    }
    explorer_select_entry(next);
}

void explorer_cycle_file() noexcept {
    explorer_step(1);
}

void explorer_scroll(int delta) noexcept {
    if (g_ui.exp_count <= CompositorState::kVisibleDirRows) {
        g_ui.exp_scroll = 0;
    } else {
        const int max_scroll = g_ui.exp_count - CompositorState::kVisibleDirRows;
        int next = g_ui.exp_scroll + delta;
        if (delta > 0 && g_ui.exp_scroll >= max_scroll) {
            next = 0;
        } else if (delta < 0 && g_ui.exp_scroll <= 0) {
            next = max_scroll;
        } else {
            next = clamp_i(next, 0, max_scroll);
        }
        g_ui.exp_scroll = next;
    }

    char nscroll[16];
    char ncount[16];
    fmt_u32(nscroll, 0, static_cast<uint32_t>(g_ui.exp_scroll), sizeof(nscroll));
    fmt_u32(ncount, 0, static_cast<uint32_t>(g_ui.exp_count), sizeof(ncount));
    char log_buf[192];
    size_t lp = 0;
    lp = fmt_str_cap(log_buf, sizeof(log_buf), lp, "[ui_explorer::scroll] dir=");
    lp = fmt_str_cap(log_buf, sizeof(log_buf), lp, g_ui.exp_dir);
    lp = fmt_str_cap(log_buf, sizeof(log_buf), lp, " scroll=");
    lp = fmt_str_cap(log_buf, sizeof(log_buf), lp, nscroll);
    lp = fmt_str_cap(log_buf, sizeof(log_buf), lp, " count=");
    lp = fmt_str_cap(log_buf, sizeof(log_buf), lp, ncount);
    lp = fmt_str_cap(log_buf, sizeof(log_buf), lp, "\n");
    fipsos::sys_write(fipsos::FD_STDOUT, log_buf, lp);
}

void explorer_execute_selected() noexcept {
    if (g_ui.exp_selected < 0 || g_ui.exp_selected >= g_ui.exp_count) return;
    const VfsEntry& e = g_ui.exp_entries[g_ui.exp_selected];
    char full_path[128];
    build_child_path(full_path, sizeof(full_path), g_ui.exp_dir, e.name);

    if (e.type == fipsos::DT_DIR) {
        explorer_load_directory(full_path);
        return;
    }
    if (e.type != fipsos::DT_REG) return;

    if (has_suffix(e.name, ".lua")) {
        const char* argv[] = {"/bin/lua", full_path, nullptr};
        const int64_t rc = exec_child_capturing("/bin/lua", argv, "/tmp/ui_exec.log", false);
        preview_vfs_file("/tmp/ui_exec.log", e.name);
        log_exec_preview("lua", full_path);
        g_ui.app_launches += 1U;
        size_t p = 0;
        p = fmt_str_cap(g_ui.last_action, sizeof(g_ui.last_action), p, "Lua ");
        p = fmt_str_cap(g_ui.last_action, sizeof(g_ui.last_action), p, e.name);
        p = fmt_str_cap(g_ui.last_action, sizeof(g_ui.last_action), p, " rc=");
        fmt_u32(g_ui.last_action, p, rc >= 0 ? static_cast<uint32_t>(rc) : 1U, sizeof(g_ui.last_action));
    } else if (has_suffix(e.name, ".c")) {
        const char* c_argv[] = {"/bin/tcc", full_path, "-o", "/tmp/ui_out.elf", nullptr};
        const int64_t c_rc = exec_child_capturing("/bin/tcc", c_argv, "/tmp/ui_exec.log", false);
        if (c_rc == 0) {
            const char* r_argv[] = {"/tmp/ui_out.elf", nullptr};
            const int64_t r_rc = exec_child_capturing("/tmp/ui_out.elf", r_argv, "/tmp/ui_exec.log", true);
            preview_vfs_file("/tmp/ui_exec.log", e.name);
            log_exec_preview("tcc", full_path);
            g_ui.app_launches += 1U;
            size_t p = 0;
            p = fmt_str_cap(g_ui.last_action, sizeof(g_ui.last_action), p, "TCC ");
            p = fmt_str_cap(g_ui.last_action, sizeof(g_ui.last_action), p, e.name);
            p = fmt_str_cap(g_ui.last_action, sizeof(g_ui.last_action), p, " rc=");
            fmt_u32(g_ui.last_action, p, r_rc >= 0 ? static_cast<uint32_t>(r_rc) : 1U, sizeof(g_ui.last_action));
        } else {
            log_exec_preview("tcc_err", full_path);
            preview_vfs_file("/tmp/ui_exec.log", e.name);
            set_cstr(g_ui.last_action, sizeof(g_ui.last_action), "TCC fallo compilacion C");
        }
    } else if (!has_text_ext(e.name)) {
        action_launch_elf(full_path, e.name);
    }
    refresh_kernel_telemetry();
}

void explorer_edit_selected() noexcept {
    if (g_ui.exp_selected < 0 || g_ui.exp_selected >= g_ui.exp_count) return;
    const VfsEntry& e = g_ui.exp_entries[g_ui.exp_selected];
    if (e.type != fipsos::DT_REG) return;
    char full_path[128];
    build_child_path(full_path, sizeof(full_path), g_ui.exp_dir, e.name);
    editor_open_file(full_path, e.name);
}

void draw_win_explorer(const FloatingWindow& w, bool focused) noexcept {
    draw_window_chrome(w, focused);
    const int cx = w.x + 14;
    const int cy = w.y + 40;
    const int cw = w.w - 28;

    const int tab_w = (cw - 18) / 4;
    for (int t = 0; t < 4; ++t) {
        const int tx = cx + t * (tab_w + 6);
        const bool active = (g_ui.exp_active_tab == t);
        draw_button(tx, cy, tab_w, 22, kDirTabs[t],
                    active ? 0x173A5E : 0x0B1526,
                    active ? 0x00F5D4 : 0x26466D,
                    active ? 0x00F5D4 : 0xCBD5E1);
    }

    const int bar_y = cy + 26;
    fill_round_rect(cx, bar_y, cw, 22, 4, 0x081120);
    stroke_round_rect(cx, bar_y, cw, 22, 4, 0x28476E);
    draw_text(g_ui.exp_status, cx + 8, bar_y + 7, 0x38BDF8, cx + cw - 8);

    const int py = cy + 52;
    const int ph = 200;
    const int list_w = 212;
    const int prev_x = cx + list_w + 8;
    const int prev_w = cw - list_w - 8;

    fill_round_rect(cx, py, list_w, ph, 5, 0x07111D);
    stroke_round_rect(cx, py, list_w, ph, 5, 0x1E3A5F);
    char lhdr[36];
    size_t hp = fmt_str_cap(lhdr, sizeof(lhdr), 0, "Entradas VFS ");
    if (g_ui.exp_count > 0) {
        const int end_i = min_i(g_ui.exp_count, g_ui.exp_scroll + CompositorState::kVisibleDirRows);
        hp = fmt_str_cap(lhdr, sizeof(lhdr), hp, "[");
        hp = fmt_u32(lhdr, hp, static_cast<uint32_t>(g_ui.exp_scroll + 1), sizeof(lhdr));
        hp = fmt_str_cap(lhdr, sizeof(lhdr), hp, "-");
        hp = fmt_u32(lhdr, hp, static_cast<uint32_t>(end_i), sizeof(lhdr));
        hp = fmt_str_cap(lhdr, sizeof(lhdr), hp, "/");
        hp = fmt_u32(lhdr, hp, static_cast<uint32_t>(g_ui.exp_count), sizeof(lhdr));
        fmt_str_cap(lhdr, sizeof(lhdr), hp, "]");
    }
    draw_text(lhdr, cx + 8, py + 7, 0xFBBF24, cx + list_w - 6);

    for (int row = 0; row < CompositorState::kVisibleDirRows; ++row) {
        const int idx = g_ui.exp_scroll + row;
        if (idx >= g_ui.exp_count) break;
        const int ry = py + 24 + row * 19;
        const bool sel = (idx == g_ui.exp_selected);
        if (sel) {
            fill_round_rect(cx + 4, ry - 2, list_w - 8, 17, 3, 0x172554);
            stroke_round_rect(cx + 4, ry - 2, list_w - 8, 17, 3, 0x38BDF8);
        }
        const VfsEntry& e = g_ui.exp_entries[idx];
        const char* tag = (e.type == fipsos::DT_DIR) ? "[D] " :
                          (e.type == fipsos::DT_CHR) ? "[C] " : "[F] ";
        const uint32_t col = (e.type == fipsos::DT_DIR) ? 0xFBBF24 :
                             (e.type == fipsos::DT_CHR) ? 0xC084FC :
                             sel ? 0x00F5D4 : 0xE2E8F0;
        draw_text(tag, cx + 8, ry + 2, col, cx + list_w - 8);
        draw_text(e.name, cx + 38, ry + 2, col, cx + list_w - 8);
    }

    fill_round_rect(prev_x, py, prev_w, ph, 5, 0x07111D);
    stroke_round_rect(prev_x, py, prev_w, ph, 5, 0x1E3A5F);
    draw_text("Vista Previa:", prev_x + 8, py + 7, 0x00F5D4, prev_x + prev_w - 8);
    draw_text(g_ui.exp_preview_title, prev_x + 116, py + 7, 0xFBBF24, prev_x + prev_w - 8);

    if (g_img_prev.active) {
        draw_image_preview(prev_x + 8, py + 26, prev_w - 16, ph - 32);
    } else {
        for (int i = 0; i < g_ui.exp_preview_line_count && i < 9; ++i) {
            draw_text(g_ui.exp_preview_lines[i], prev_x + 8, py + 26 + i * 19, 0x7AE582, prev_x + prev_w - 8);
        }
    }

    const int by = w.y + w.h - 36;
    draw_button(w.x + 14,  by, 82,  26, "Refrescar",      0x064E3B, 0x34D399, 0xF8FAFC);
    draw_button(w.x + 100, by, 86,  26, "Sig. Arch.",     0x1E3A8A, 0x60A5FA, 0xF8FAFC);
    draw_button(w.x + 190, by, 86,  26, "Pag. Abajo",     0x4C1D95, 0xC084FC, 0xF8FAFC);
    draw_button(w.x + 280, by, 110, 26, "Ejecutar [ENT]", 0x7C2D12, 0xFB923C, 0xF8FAFC);
    draw_button(w.x + 394, by, 96,  26, "Editar [E]",     0x0F766E, 0x2DD4BF, 0xF8FAFC);
    draw_button(w.x + 494, by, 74,  26, "Nuevo [N]",      0x1E40AF, 0x38BDF8, 0xF8FAFC);
}

bool explorer_handle_window_click(const FloatingWindow& w, int mx, int my) noexcept {
    const int by = w.y + w.h - 36;
    const int cx = w.x + 14;
    const int cy = w.y + 40;
    const int cw = w.w - 28;
    const int tab_w = (cw - 18) / 4;
    for (int t = 0; t < 4; ++t) {
        if (pt_in_rect(mx, my, cx + t * (tab_w + 6), cy, tab_w, 22)) {
            explorer_switch_dir_tab(t);
            return true;
        }
    }
    const int py = cy + 52;
    const int list_w = 212;
    if (pt_in_rect(mx, my, cx, py + 22, list_w, CompositorState::kVisibleDirRows * 19)) {
        const int row = (my - (py + 22)) / 19;
        if (row >= 0 && row < CompositorState::kVisibleDirRows && (g_ui.exp_scroll + row) < g_ui.exp_count) {
            explorer_select_entry(g_ui.exp_scroll + row);
            return true;
        }
    }
    if (pt_in_rect(mx, my, w.x + 14,  by, 82,  26)) { explorer_load_directory(g_ui.exp_dir); return true; }
    if (pt_in_rect(mx, my, w.x + 100, by, 86,  26)) { explorer_cycle_file(); return true; }
    if (pt_in_rect(mx, my, w.x + 190, by, 86,  26)) { explorer_scroll(CompositorState::kVisibleDirRows); return true; }
    if (pt_in_rect(mx, my, w.x + 280, by, 110, 26)) { explorer_execute_selected(); return true; }
    if (pt_in_rect(mx, my, w.x + 394, by, 96,  26)) { explorer_edit_selected(); return true; }
    if (pt_in_rect(mx, my, w.x + 494, by, 74,  26)) { editor_new_file(g_ui.exp_dir); return true; }
    return false;
}

} // namespace fipsos::ui
