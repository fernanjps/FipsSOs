#include "ui_editor.hpp"
#include "ui_wm.hpp"
#include "ui_apps.hpp"
#include "ui_clipboard.hpp"

namespace fipsos::ui {

namespace {

constexpr int kEdMaxLines   = 128;
constexpr int kEdMaxCols    = 96;
constexpr int kEdVisibleRow = 12;

struct EditorState {
    char path[128];
    char title[64];
    char status[80];
    char output[160];
    char lines[kEdMaxLines][kEdMaxCols];
    int  line_count;
    int  cur_row;
    int  cur_col;
    int  scroll_row;
    bool dirty;
    bool initialized;
};

EditorState s_ed;

bool ed_has_suffix(const char* s, const char* suf) noexcept {
    const size_t sl = str_len(s);
    const size_t fl = str_len(suf);
    if (sl < fl) return false;
    for (size_t i = 0; i < fl; ++i) {
        if (s[sl - fl + i] != suf[i]) return false;
    }
    return true;
}

void ed_clamp_scroll() noexcept {
    if (s_ed.cur_row < s_ed.scroll_row) {
        s_ed.scroll_row = s_ed.cur_row;
    } else if (s_ed.cur_row >= s_ed.scroll_row + kEdVisibleRow) {
        s_ed.scroll_row = s_ed.cur_row - (kEdVisibleRow - 1);
    }
    if (s_ed.scroll_row < 0) s_ed.scroll_row = 0;
}

void ed_ensure_init() noexcept {
    if (s_ed.initialized) return;
    s_ed.initialized = true;
    set_cstr(s_ed.path, sizeof(s_ed.path), "/mnt/fat0/nota.txt");
    set_cstr(s_ed.title, sizeof(s_ed.title), "nota.txt");
    set_cstr(s_ed.status, sizeof(s_ed.status), "Listo. Selecciona un archivo en Explorador.");
    s_ed.output[0] = '\0';
    s_ed.line_count = 1;
    s_ed.lines[0][0] = '\0';
    s_ed.cur_row = 0;
    s_ed.cur_col = 0;
    s_ed.scroll_row = 0;
    s_ed.dirty = false;
}

void ed_load_output_preview(const char* log_path) noexcept {
    s_ed.output[0] = '\0';
    const int64_t fd = fipsos::sys_open(log_path, fipsos::O_RDONLY);
    if (fd < 0) return;
    char buf[160];
    const int64_t n = fipsos::sys_read(static_cast<uint64_t>(fd), buf, sizeof(buf) - 1U);
    fipsos::sys_close(static_cast<uint64_t>(fd));
    if (n <= 0) return;
    size_t out_i = 0;
    for (int64_t i = 0; i < n && out_i + 1 < sizeof(s_ed.output); ++i) {
        char ch = buf[i];
        if (ch == '\n' || ch == '\r' || ch == '\t') ch = ' ';
        if (static_cast<uint8_t>(ch) >= 0x20 && static_cast<uint8_t>(ch) <= 0x7E) {
            s_ed.output[out_i++] = ch;
        }
    }
    s_ed.output[out_i] = '\0';
}

} // namespace

void editor_open_file(const char* path, const char* display_name) noexcept {
    ed_ensure_init();
    set_cstr(s_ed.path, sizeof(s_ed.path), path);
    set_cstr(s_ed.title, sizeof(s_ed.title), (display_name && display_name[0]) ? display_name : path);
    s_ed.output[0] = '\0';
    s_ed.cur_row = 0;
    s_ed.cur_col = 0;
    s_ed.scroll_row = 0;
    s_ed.dirty = false;
    s_ed.line_count = 1;
    s_ed.lines[0][0] = '\0';

    static char raw[8192];
    const int64_t fd = fipsos::sys_open(path, fipsos::O_RDONLY);
    if (fd >= 0) {
        const int64_t n = fipsos::sys_read(static_cast<uint64_t>(fd), raw, sizeof(raw) - 1U);
        fipsos::sys_close(static_cast<uint64_t>(fd));
        if (n > 0) {
            int r = 0;
            int c = 0;
            for (int64_t i = 0; i < n && r < kEdMaxLines; ++i) {
                const char ch = raw[i];
                if (ch == '\r') continue;
                if (ch == '\n') {
                    s_ed.lines[r][c] = '\0';
                    if (r + 1 < kEdMaxLines) {
                        r++;
                        c = 0;
                        s_ed.lines[r][0] = '\0';
                    } else {
                        break;
                    }
                } else if (c + 1 < kEdMaxCols && static_cast<uint8_t>(ch) >= 0x20) {
                    s_ed.lines[r][c++] = ch;
                }
            }
            s_ed.lines[r][c] = '\0';
            s_ed.line_count = r + 1;
        }
        size_t p = fmt_str_cap(s_ed.status, sizeof(s_ed.status), 0, "Cargado: ");
        fmt_str_cap(s_ed.status, sizeof(s_ed.status), p, s_ed.title);
    } else {
        size_t p = fmt_str_cap(s_ed.status, sizeof(s_ed.status), 0, "Nuevo archivo: ");
        fmt_str_cap(s_ed.status, sizeof(s_ed.status), p, s_ed.title);
    }

    char log_buf[192];
    size_t lp = fmt_str_cap(log_buf, sizeof(log_buf), 0, "[ui_editor] open path=");
    lp = fmt_str_cap(log_buf, sizeof(log_buf), lp, s_ed.path);
    lp = fmt_str_cap(log_buf, sizeof(log_buf), lp, "\n");
    fipsos::sys_write(fipsos::FD_STDOUT, log_buf, lp);
    focus_window(5);
}

void editor_save_file() noexcept {
    ed_ensure_init();
    static char save_buf[kEdMaxLines * (kEdMaxCols + 2)];
    size_t total = 0;
    for (int r = 0; r < s_ed.line_count; ++r) {
        const size_t len = str_len(s_ed.lines[r]);
        for (size_t c = 0; c < len && total + 2 < sizeof(save_buf); ++c) {
            save_buf[total++] = s_ed.lines[r][c];
        }
        if (r + 1 < s_ed.line_count && total + 2 < sizeof(save_buf)) {
            save_buf[total++] = '\n';
        }
    }
    if (total > 0 && save_buf[total - 1] != '\n' && total + 1 < sizeof(save_buf)) {
        save_buf[total++] = '\n';
    }

    const int64_t fd = fipsos::sys_openat(fipsos::AT_FDCWD, s_ed.path, fipsos::O_WRONLY | fipsos::O_CREAT | fipsos::O_TRUNC, 0644);
    if (fd < 0) {
        set_cstr(s_ed.status, sizeof(s_ed.status), "Error al guardar (permisos/ruta)");
        return;
    }
    const int64_t wr = (total > 0) ? fipsos::sys_write(static_cast<uint64_t>(fd), save_buf, total) : 0;
    fipsos::sys_close(static_cast<uint64_t>(fd));
    if (wr < 0) {
        set_cstr(s_ed.status, sizeof(s_ed.status), "Error de escritura en disco");
        return;
    }
    s_ed.dirty = false;
    size_t p = fmt_str_cap(s_ed.status, sizeof(s_ed.status), 0, "Guardado OK (");
    p = fmt_u32(s_ed.status, p, static_cast<uint32_t>(wr), sizeof(s_ed.status));
    fmt_str_cap(s_ed.status, sizeof(s_ed.status), p, " B)");

    char log_buf[192];
    size_t lp = fmt_str_cap(log_buf, sizeof(log_buf), 0, "[ui_editor] save path=");
    lp = fmt_str_cap(log_buf, sizeof(log_buf), lp, s_ed.path);
    lp = fmt_str_cap(log_buf, sizeof(log_buf), lp, " bytes=");
    lp = fmt_u32(log_buf, lp, static_cast<uint32_t>(wr), sizeof(log_buf));
    lp = fmt_str_cap(log_buf, sizeof(log_buf), lp, "\n");
    fipsos::sys_write(fipsos::FD_STDOUT, log_buf, lp);
}

void editor_save_and_run() noexcept {
    editor_save_file();
    if (s_ed.dirty) return;
    int64_t rc = -1;
    if (ed_has_suffix(s_ed.path, ".c")) {
        const char* c_argv[] = {"/bin/tcc", s_ed.path, "-o", "/tmp/ed_out.elf", nullptr};
        rc = exec_child_capturing("/bin/tcc", c_argv, "/tmp/ed_exec.log", false);
        if (rc == 0) {
            const char* r_argv[] = {"/tmp/ed_out.elf", nullptr};
            rc = exec_child_capturing("/tmp/ed_out.elf", r_argv, "/tmp/ed_exec.log", true);
        }
    } else if (ed_has_suffix(s_ed.path, ".lua")) {
        const char* l_argv[] = {"/bin/lua", s_ed.path, nullptr};
        rc = exec_child_capturing("/bin/lua", l_argv, "/tmp/ed_exec.log", false);
    } else {
        set_cstr(s_ed.output, sizeof(s_ed.output), "Guardado (solo .c y .lua son ejecutables)");
        return;
    }
    ed_load_output_preview("/tmp/ed_exec.log");
    size_t p = fmt_str_cap(s_ed.status, sizeof(s_ed.status), 0, "Ejecutado rc=");
    fmt_u32(s_ed.status, p, rc >= 0 ? static_cast<uint32_t>(rc) : 1U, sizeof(s_ed.status));

    char log_buf[192];
    size_t lp = fmt_str_cap(log_buf, sizeof(log_buf), 0, "[ui_editor] run path=");
    lp = fmt_str_cap(log_buf, sizeof(log_buf), lp, s_ed.path);
    lp = fmt_str_cap(log_buf, sizeof(log_buf), lp, " out=");
    lp = fmt_str_cap(log_buf, sizeof(log_buf), lp, s_ed.output);
    lp = fmt_str_cap(log_buf, sizeof(log_buf), lp, "\n");
    fipsos::sys_write(fipsos::FD_STDOUT, log_buf, lp);
    refresh_kernel_telemetry();
}

void editor_new_file(const char* dir) noexcept {
    ed_ensure_init();
    const char* base = (dir && dir[0]) ? dir : "/mnt/fat0";
    char cand[128];
    const size_t bl = str_len(base);
    auto make_cand = [&](const char* name) {
        size_t p = fmt_str_cap(cand, sizeof(cand), 0, base);
        if (bl > 0 && base[bl - 1] != '/') p = fmt_str_cap(cand, sizeof(cand), p, "/");
        fmt_str_cap(cand, sizeof(cand), p, name);
    };
    make_cand("nuevo_v49.c");
    int64_t chk = fipsos::sys_open(cand, fipsos::O_RDONLY);
    if (chk >= 0) {
        fipsos::sys_close(static_cast<uint64_t>(chk));
        for (int n = 1; n <= 9; ++n) {
            char nm[16] = "nuevo_0.c";
            nm[6] = static_cast<char>('0' + n);
            make_cand(nm);
            chk = fipsos::sys_open(cand, fipsos::O_RDONLY);
            if (chk < 0) break;
            fipsos::sys_close(static_cast<uint64_t>(chk));
        }
    }
    set_cstr(s_ed.path, sizeof(s_ed.path), cand);
    const char* slash = cand;
    for (const char* q = cand; *q; ++q) {
        if (*q == '/') slash = q + 1;
    }
    set_cstr(s_ed.title, sizeof(s_ed.title), slash);
    set_cstr(s_ed.lines[0], kEdMaxCols, "#include <stdio.h>");
    set_cstr(s_ed.lines[1], kEdMaxCols, "int main(void) {");
    set_cstr(s_ed.lines[2], kEdMaxCols, "  printf(\"Hola FipSOs v5.5\\n\");");
    set_cstr(s_ed.lines[3], kEdMaxCols, "  return 0;");
    set_cstr(s_ed.lines[4], kEdMaxCols, "}");
    s_ed.line_count = 5;
    s_ed.cur_row = 2;
    s_ed.cur_col = 2;
    s_ed.scroll_row = 0;
    s_ed.output[0] = '\0';
    editor_save_file();
    if (!s_ed.dirty) set_cstr(s_ed.status, sizeof(s_ed.status), "Nuevo archivo creado");
    focus_window(5);
}

void editor_scroll(int delta) noexcept {
    ed_ensure_init();
    const int max_s = (s_ed.line_count > kEdVisibleRow) ? (s_ed.line_count - kEdVisibleRow) : 0;
    s_ed.scroll_row = clamp_i(s_ed.scroll_row + delta, 0, max_s);
}

void editor_handle_arrow(uint8_t code) noexcept {
    ed_ensure_init();
    if (code == 'A' && s_ed.cur_row > 0) s_ed.cur_row--;
    else if (code == 'B' && s_ed.cur_row + 1 < s_ed.line_count) s_ed.cur_row++;
    else if (code == 'D' && s_ed.cur_col > 0) s_ed.cur_col--;
    else if (code == 'C') {
        const int len = static_cast<int>(str_len(s_ed.lines[s_ed.cur_row]));
        if (s_ed.cur_col < len) s_ed.cur_col++;
    }
    const int len = static_cast<int>(str_len(s_ed.lines[s_ed.cur_row]));
    if (s_ed.cur_col > len) s_ed.cur_col = len;
    ed_clamp_scroll();
}

void editor_handle_key(uint8_t key) noexcept {
    ed_ensure_init();
    if (key == 0x03U) { // Ctrl+C: copiar linea actual
        clipboard_set_cstr(s_ed.lines[s_ed.cur_row]);
        set_cstr(s_ed.status, sizeof(s_ed.status), "Copiado al portapapeles");
        return;
    }
    if (key == 0x16U) { // Ctrl+V: pegar en el cursor
        const char* clip = clipboard_get();
        const int clen = static_cast<int>(clipboard_len());
        char* ln = s_ed.lines[s_ed.cur_row];
        const int cur_len = static_cast<int>(str_len(ln));
        int ins = 0;
        while (ins < clen && clip[ins] != '\n' && clip[ins] != '\r' &&
               cur_len + ins + 1 < kEdMaxCols) {
            ++ins;
        }
        if (ins > 0) {
            for (int i = cur_len; i >= s_ed.cur_col; --i) {
                ln[i + ins] = ln[i];
            }
            for (int i = 0; i < ins; ++i) {
                ln[s_ed.cur_col + i] = clip[i];
            }
            s_ed.cur_col += ins;
            s_ed.dirty = true;
            set_cstr(s_ed.status, sizeof(s_ed.status), "Pegado desde portapapeles");
        }
        return;
    }
    if (key == 0x13U) { // Ctrl+S: guardar
        editor_save_file();
        return;
    }
    if (key == 0x12U) { // Ctrl+R: guardar y ejecutar
        editor_save_and_run();
        return;
    }
    if (key == '\n' || key == '\r') {
        if (s_ed.line_count < kEdMaxLines) {
            for (int r = s_ed.line_count; r > s_ed.cur_row + 1; --r) {
                set_cstr(s_ed.lines[r], kEdMaxCols, s_ed.lines[r - 1]);
            }
            s_ed.line_count++;
            set_cstr(s_ed.lines[s_ed.cur_row + 1], kEdMaxCols, &s_ed.lines[s_ed.cur_row][s_ed.cur_col]);
            s_ed.lines[s_ed.cur_row][s_ed.cur_col] = '\0';
            s_ed.cur_row++;
            s_ed.cur_col = 0;
            s_ed.dirty = true;
            ed_clamp_scroll();
        }
        return;
    }
    if (key == 0x08U || key == 0x7FU) {
        if (s_ed.cur_col > 0) {
            char* ln = s_ed.lines[s_ed.cur_row];
            const int len = static_cast<int>(str_len(ln));
            for (int i = s_ed.cur_col - 1; i < len; ++i) {
                ln[i] = ln[i + 1];
            }
            s_ed.cur_col--;
            s_ed.dirty = true;
        } else if (s_ed.cur_row > 0) {
            char* prev = s_ed.lines[s_ed.cur_row - 1];
            char* curr = s_ed.lines[s_ed.cur_row];
            const int prev_len = static_cast<int>(str_len(prev));
            const int curr_len = static_cast<int>(str_len(curr));
            const int copy_n = min_i(curr_len, (kEdMaxCols - 1) - prev_len);
            for (int i = 0; i < copy_n; ++i) prev[prev_len + i] = curr[i];
            prev[prev_len + copy_n] = '\0';
            for (int r = s_ed.cur_row; r + 1 < s_ed.line_count; ++r) {
                set_cstr(s_ed.lines[r], kEdMaxCols, s_ed.lines[r + 1]);
            }
            s_ed.line_count--;
            s_ed.cur_row--;
            s_ed.cur_col = prev_len;
            s_ed.dirty = true;
            ed_clamp_scroll();
        }
        return;
    }
    if (key == '\t') {
        editor_handle_key(' ');
        editor_handle_key(' ');
        return;
    }
    if (key >= 0x20U && key <= 0x7EU) {
        char* ln = s_ed.lines[s_ed.cur_row];
        const int len = static_cast<int>(str_len(ln));
        if (len + 1 < kEdMaxCols) {
            for (int i = len; i >= s_ed.cur_col; --i) {
                ln[i + 1] = ln[i];
            }
            ln[s_ed.cur_col] = static_cast<char>(key);
            s_ed.cur_col++;
            s_ed.dirty = true;
        }
    }
}

void draw_win_editor(const FloatingWindow& w, bool focused) noexcept {
    ed_ensure_init();
    draw_window_chrome(w, focused);
    const int cx = w.x + 14;
    const int cy = w.y + 40;
    const int cw = w.w - 28;

    fill_round_rect(cx, cy, cw, 22, 4, 0x081120);
    stroke_round_rect(cx, cy, cw, 22, 4, 0x28476E);
    char hdr[96];
    size_t hp = fmt_str_cap(hdr, sizeof(hdr), 0, s_ed.path);
    if (s_ed.dirty) hp = fmt_str_cap(hdr, sizeof(hdr), hp, " [*]");
    hp = fmt_str_cap(hdr, sizeof(hdr), hp, " | ");
    fmt_str_cap(hdr, sizeof(hdr), hp, s_ed.status);
    draw_text(hdr, cx + 8, cy + 7, s_ed.dirty ? 0xFBBF24 : 0x38BDF8, cx + cw - 8);

    const int vp_y = cy + 28;
    const int vp_h = 224;
    fill_round_rect(cx, vp_y, cw, vp_h, 5, 0x060D1A);
    stroke_round_rect(cx, vp_y, cw, vp_h, 5, 0x1E3A5F);

    for (int row = 0; row < kEdVisibleRow; ++row) {
        const int idx = s_ed.scroll_row + row;
        if (idx >= s_ed.line_count) break;
        const int ry = vp_y + 8 + row * 18;
        if (idx == s_ed.cur_row) {
            fill_rect(cx + 4, ry - 2, cw - 8, 16, 0x12223C);
        }
        char lnum[8];
        fmt_u32_pad2(lnum, 0, static_cast<uint32_t>(idx + 1), sizeof(lnum));
        draw_text(lnum, cx + 8, ry + 2, 0x64748B, cx + 32);
        draw_text(s_ed.lines[idx], cx + 36, ry + 2, 0xE2E8F0, cx + cw - 8);
        if (idx == s_ed.cur_row) {
            const int cur_x = cx + 36 + s_ed.cur_col * 8;
            if (cur_x + 7 < cx + cw - 6) {
                fill_rect(cur_x, ry + 11, 7, 2, 0x00F5D4);
            }
        }
    }

    const int out_y = vp_y + vp_h + 6;
    fill_round_rect(cx, out_y, cw, 24, 4, 0x071422);
    stroke_round_rect(cx, out_y, cw, 24, 4, 0x1E3A5F);
    draw_text("Salida:", cx + 8, out_y + 8, 0x00F5D4, cx + 70);
    draw_text(s_ed.output[0] ? s_ed.output : "(sin ejecucion reciente)", cx + 72, out_y + 8, 0x7AE582, cx + cw - 8);

    const int by = w.y + w.h - 36;
    draw_button(w.x + 14,  by, 64,  26, "Nuevo",              0x0F766E, 0x2DD4BF, 0xF8FAFC);
    draw_button(w.x + 84,  by, 86,  26, "Guardar",            0x064E3B, 0x34D399, 0xF8FAFC);
    draw_button(w.x + 176, by, 164, 26, "Guardar y Ejecutar", 0x7C2D12, 0xFB923C, 0xF8FAFC);
    draw_button(w.x + 346, by, 66,  26, "Subir",              0x1E3A8A, 0x60A5FA, 0xF8FAFC);
    draw_button(w.x + 418, by, 66,  26, "Bajar",              0x1E3A8A, 0x60A5FA, 0xF8FAFC);
    draw_button(w.x + 490, by, 96,  26, "Explorador",         0x311042, 0xC084FC, 0xF8FAFC);
}

bool editor_handle_window_click(const FloatingWindow& w, int mx, int my) noexcept {
    ed_ensure_init();
    const int by = w.y + w.h - 36;
    if (pt_in_rect(mx, my, w.x + 14,  by, 64,  26)) { editor_new_file("/mnt/fat0"); return true; }
    if (pt_in_rect(mx, my, w.x + 84,  by, 86,  26)) { editor_save_file(); return true; }
    if (pt_in_rect(mx, my, w.x + 176, by, 164, 26)) { editor_save_and_run(); return true; }
    if (pt_in_rect(mx, my, w.x + 346, by, 66,  26)) { editor_scroll(-4); return true; }
    if (pt_in_rect(mx, my, w.x + 418, by, 66,  26)) { editor_scroll(4); return true; }
    if (pt_in_rect(mx, my, w.x + 490, by, 96,  26)) { focus_window(2); return true; }

    const int vp_y = w.y + 40 + 28;
    const int vp_h = 224;
    if (pt_in_rect(mx, my, w.x + 14, vp_y, w.w - 28, vp_h)) {
        const int row = (my - (vp_y + 8)) / 18;
        const int idx = s_ed.scroll_row + row;
        if (idx >= 0 && idx < s_ed.line_count) {
            s_ed.cur_row = idx;
            const int col = clamp_i((mx - (w.x + 14 + 36)) / 8, 0, static_cast<int>(str_len(s_ed.lines[idx])));
            s_ed.cur_col = col;
            return true;
        }
    }
    return false;
}

} // namespace fipsos::ui
