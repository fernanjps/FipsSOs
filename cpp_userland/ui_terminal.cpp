#include "ui_terminal.hpp"
#include "ui_wm.hpp"
#include "ui_explorer.hpp"
#include "ui_apps.hpp"
#include "ui_clipboard.hpp"

namespace fipsos::ui {

namespace {

constexpr const char* kTermLog = "/tmp/ui_term.log";
constexpr int kTermVisibleRows = 16;

void join_path(char* dst, size_t cap, const char* base, const char* name) noexcept {
    if (name[0] == '/') {
        set_cstr(dst, cap, name);
        return;
    }
    size_t p = 0;
    p = fmt_str_cap(dst, cap, p, base);
    if (p > 0 && dst[p - 1] != '/' && p + 1 < cap) {
        dst[p++] = '/';
        dst[p]   = '\0';
    }
    fmt_str_cap(dst, cap, p, name);
}

bool file_exists_readable(const char* path) noexcept {
    const int64_t fd = fipsos::sys_openat(fipsos::AT_FDCWD, path, fipsos::O_RDONLY, 0);
    if (fd >= 0) {
        fipsos::sys_close(static_cast<uint64_t>(fd));
        return true;
    }
    return false;
}

void append_text_block(const char* buf, size_t len) noexcept {
    char line[76];
    size_t lp = 0;
    for (size_t i = 0; i < len; ++i) {
        const char ch = buf[i];
        if (ch == '\r') continue;
        if (ch == '\n') {
            line[lp] = '\0';
            terminal_append_line(line);
            lp = 0;
        } else {
            if (lp + 1 >= sizeof(line)) {
                line[lp] = '\0';
                terminal_append_line(line);
                lp = 0;
            }
            line[lp++] = (ch >= 0x20 && ch < 0x7F) ? ch : '.';
        }
    }
    if (lp > 0) {
        line[lp] = '\0';
        terminal_append_line(line);
    }
}

void cmd_ls_dir(const char* dir) noexcept {
    const int64_t fd = fipsos::sys_openat(fipsos::AT_FDCWD, dir, fipsos::O_RDONLY, 0);
    if (fd < 0) {
        terminal_append_line("ls: directorio no accesible");
        return;
    }
    uint8_t dbuf[1024];
    int shown = 0;
    while (true) {
        const int64_t nread = fipsos::sys_getdents64(static_cast<uint64_t>(fd), dbuf, sizeof(dbuf));
        if (nread <= 0) break;
        size_t pos = 0;
        while (pos + 19 <= static_cast<size_t>(nread)) {
            const uint16_t reclen = static_cast<uint16_t>(dbuf[pos + 16]) |
                                    (static_cast<uint16_t>(dbuf[pos + 17]) << 8);
            if (reclen < 19 || pos + reclen > static_cast<size_t>(nread)) break;
            const uint8_t dtype = dbuf[pos + 18];
            const char* name    = reinterpret_cast<const char*>(dbuf + pos + 19);
            if (!str_eq(name, ".") && !str_eq(name, "..")) {
                char row[76];
                size_t p = 0;
                p = fmt_str_cap(row, sizeof(row), p, (dtype == fipsos::DT_DIR) ? "[DIR] " : "[FIL] ");
                fmt_str_cap(row, sizeof(row), p, name);
                terminal_append_line(row);
                ++shown;
            }
            pos += reclen;
        }
    }
    fipsos::sys_close(static_cast<uint64_t>(fd));
    if (shown == 0) terminal_append_line("(directorio vacio)");
}

void cmd_cat_file(const char* path) noexcept {
    const int64_t fd = fipsos::sys_openat(fipsos::AT_FDCWD, path, fipsos::O_RDONLY, 0);
    if (fd < 0) {
        terminal_append_line("cat: no se pudo abrir archivo");
        return;
    }
    char buf[1024];
    const int64_t n = fipsos::sys_read(static_cast<uint64_t>(fd), buf, sizeof(buf) - 1);
    fipsos::sys_close(static_cast<uint64_t>(fd));
    if (n > 0) {
        append_text_block(buf, static_cast<size_t>(n));
    } else {
        terminal_append_line("(archivo vacio)");
    }
}

void print_cstr(const char* s) noexcept {
    fipsos::sys_write(fipsos::FD_STDOUT, s, str_len(s));
}

void log_term_serial(const char* cmdline) noexcept {
    fipsos::print_literal("[ui_terminal::exec] ");
    print_cstr(cmdline);
    fipsos::print_literal("\n");
}

} // namespace

void terminal_append_line(const char* line) noexcept {
    if (g_ui.term_line_count < CompositorState::kTermMaxLines) {
        set_cstr(g_ui.term_lines[g_ui.term_line_count], 76, line);
        g_ui.term_line_count += 1;
    } else {
        for (int i = 1; i < CompositorState::kTermMaxLines; ++i) {
            set_cstr(g_ui.term_lines[i - 1], 76, g_ui.term_lines[i]);
        }
        set_cstr(g_ui.term_lines[CompositorState::kTermMaxLines - 1], 76, line);
    }
    if (g_ui.term_line_count > kTermVisibleRows) {
        g_ui.term_scroll = g_ui.term_line_count - kTermVisibleRows;
    } else {
        g_ui.term_scroll = 0;
    }
}

void terminal_init() noexcept {
    set_cstr(g_ui.term_cwd, sizeof(g_ui.term_cwd), "/mnt/fat0");
    g_ui.term_input[0]   = '\0';
    g_ui.term_input_len  = 0;
    g_ui.term_line_count = 0;
    g_ui.term_scroll     = 0;
    terminal_append_line("FipSOs v6.3 Terminal Integrada (C++20 | /mnt/fat0)");
    terminal_append_line("Escribe 'help' para comandos o 'lua'/'tcc'/'ls'/'theme'.");
}

void terminal_flush_keys() noexcept {
    if (g_ui.fb_fd < 0) return;
    for (int i = 0; i < 32; ++i) {
        if (fipsos::sys_ioctl(static_cast<uint64_t>(g_ui.fb_fd), fipsos::FB_IOCTL_POLL_KEY, 0ULL) <= 0) {
            break;
        }
    }
}

void terminal_scroll(int delta) noexcept {
    const int max_scroll = max_i(0, g_ui.term_line_count - kTermVisibleRows);
    g_ui.term_scroll = clamp_i(g_ui.term_scroll + delta, 0, max_scroll);
}

void terminal_exec_command(const char* cmdline) noexcept {
    char prompt_line[76];
    size_t pp = 0;
    pp = fmt_str_cap(prompt_line, sizeof(prompt_line), pp, "$ ");
    fmt_str_cap(prompt_line, sizeof(prompt_line), pp, cmdline);
    terminal_append_line(prompt_line);

    char work[96];
    set_cstr(work, sizeof(work), cmdline);
    const char* argv[16];
    int argc = 0;
    size_t i = 0;
    while (work[i] != '\0' && argc < 14) {
        while (work[i] == ' ' || work[i] == '\t') ++i;
        if (work[i] == '\0') break;
        argv[argc++] = work + i;
        while (work[i] != '\0' && work[i] != ' ' && work[i] != '\t') ++i;
        if (work[i] != '\0') work[i++] = '\0';
    }
    if (argc == 0) return;

    const char* cmd = argv[0];
    log_term_serial(cmdline);

    if (str_eq(cmd, "help")) {
        terminal_append_line("Comandos: help, clear, pwd, cd <dir>, ls [dir], cat <arch>");
        terminal_append_line("  theme <aurora|slate>, savecfg, sensors, lua, tcc, uname, ps");
        return;
    }
    if (str_eq(cmd, "clear")) {
        g_ui.term_line_count = 0;
        g_ui.term_scroll     = 0;
        return;
    }
    if (str_eq(cmd, "pwd")) {
        terminal_append_line(g_ui.term_cwd);
        return;
    }
    if (str_eq(cmd, "cd")) {
        if (argc < 2) {
            set_cstr(g_ui.term_cwd, sizeof(g_ui.term_cwd), "/mnt/fat0");
        } else {
            char target[64];
            join_path(target, sizeof(target), g_ui.term_cwd, argv[1]);
            const int64_t dfd = fipsos::sys_openat(fipsos::AT_FDCWD, target, fipsos::O_RDONLY, 0);
            if (dfd >= 0) {
                fipsos::sys_close(static_cast<uint64_t>(dfd));
                set_cstr(g_ui.term_cwd, sizeof(g_ui.term_cwd), target);
            } else {
                terminal_append_line("cd: directorio invalido");
            }
        }
        return;
    }
    if (str_eq(cmd, "ls")) {
        char target[64];
        if (argc >= 2) join_path(target, sizeof(target), g_ui.term_cwd, argv[1]);
        else           set_cstr(target, sizeof(target), g_ui.term_cwd);
        cmd_ls_dir(target);
        return;
    }
    if (str_eq(cmd, "cat") && argc >= 2) {
        char target[64];
        join_path(target, sizeof(target), g_ui.term_cwd, argv[1]);
        cmd_cat_file(target);
        return;
    }
    if (str_eq(cmd, "sensors")) {
        cmd_cat_file("/sys/fipsos/hwmon");
        return;
    }
    if (str_eq(cmd, "theme")) {
        if (argc >= 2 && str_eq(argv[1], "slate")) g_ui.theme_id = 1U;
        else if (argc >= 2 && str_eq(argv[1], "aurora")) g_ui.theme_id = 0U;
        else g_ui.theme_id ^= 1U;
        init_wallpaper_luts();
        set_cstr(g_ui.cfg_status, sizeof(g_ui.cfg_status),
                 (g_ui.theme_id == 1U) ? "Tema: Dark Slate" : "Tema: Aurora");
        terminal_append_line(g_ui.cfg_status);
        return;
    }
    if (str_eq(cmd, "savecfg")) {
        save_desktop_config();
        terminal_append_line("Guardado en /mnt/fat0/fipsos.cfg");
        return;
    }

    char prog[80];
    const char* run_argv[16];
    bool resolved = false;
    if (cmd[0] == '/' && file_exists_readable(cmd)) {
        set_cstr(prog, sizeof(prog), cmd);
        resolved = true;
    } else {
        join_path(prog, sizeof(prog), "/bin", cmd);
        if (file_exists_readable(prog)) {
            resolved = true;
        } else {
            size_t lp = str_len(prog);
            fmt_str_cap(prog, sizeof(prog), lp, ".elf");
            if (file_exists_readable(prog)) resolved = true;
        }
        if (!resolved) {
            join_path(prog, sizeof(prog), "/mnt/fat0", cmd);
            if (file_exists_readable(prog)) resolved = true;
        }
    }

    if (resolved) {
        run_argv[0] = prog;
        for (int a = 1; a < argc; ++a) run_argv[a] = argv[a];
        run_argv[argc] = nullptr;
    } else {
        set_cstr(prog, sizeof(prog), "/bin/busybox");
        run_argv[0] = prog;
        for (int a = 0; a < argc; ++a) run_argv[a + 1] = argv[a];
        run_argv[argc + 1] = nullptr;
    }

    const int64_t rc = exec_child_capturing(prog, run_argv, kTermLog, false);
    const int64_t lfd = fipsos::sys_openat(fipsos::AT_FDCWD, kTermLog, fipsos::O_RDONLY, 0);
    if (lfd >= 0) {
        char out[1024];
        const int64_t n = fipsos::sys_read(static_cast<uint64_t>(lfd), out, sizeof(out) - 1);
        fipsos::sys_close(static_cast<uint64_t>(lfd));
        if (n > 0) {
            append_text_block(out, static_cast<size_t>(n));
        } else if (rc != 0) {
            terminal_append_line("Comando finalizado con error");
        }
    }
    refresh_kernel_telemetry();
}

void terminal_handle_key(uint8_t key) noexcept {
    if (key == 0x03U) { // Ctrl+C: copiar entrada actual o ultima linea
        if (g_ui.term_input_len > 0) {
            clipboard_set_cstr(g_ui.term_input);
        } else if (g_ui.term_line_count > 0) {
            clipboard_set_cstr(g_ui.term_lines[g_ui.term_line_count - 1]);
        }
        return;
    }
    if (key == 0x16U) { // Ctrl+V: pegar en la entrada
        const char* clip = clipboard_get();
        const size_t clen = clipboard_len();
        for (size_t i = 0; i < clen; ++i) {
            const uint8_t ch = static_cast<uint8_t>(clip[i]);
            if (ch >= 0x20U && ch < 0x7FU &&
                g_ui.term_input_len + 1 < static_cast<int>(sizeof(g_ui.term_input))) {
                g_ui.term_input[g_ui.term_input_len++] = static_cast<char>(ch);
                g_ui.term_input[g_ui.term_input_len] = '\0';
            }
        }
        return;
    }
    if (key == '\n' || key == '\r') {
        if (g_ui.term_input_len > 0) {
            char cmd[96];
            set_cstr(cmd, sizeof(cmd), g_ui.term_input);
            g_ui.term_input[0]  = '\0';
            g_ui.term_input_len = 0;
            terminal_exec_command(cmd);
        }
        return;
    }
    if (key == 0x08U || key == 0x7FU) {
        if (g_ui.term_input_len > 0) {
            g_ui.term_input_len -= 1;
            g_ui.term_input[g_ui.term_input_len] = '\0';
        }
        return;
    }
    if (key >= 0x20U && key < 0x7FU) {
        if (g_ui.term_input_len + 1 < static_cast<int>(sizeof(g_ui.term_input))) {
            g_ui.term_input[g_ui.term_input_len++] = static_cast<char>(key);
            g_ui.term_input[g_ui.term_input_len]   = '\0';
        }
    }
}

void draw_win_terminal(const FloatingWindow& w, bool focused) noexcept {
    draw_window_chrome(w, focused);
    const int bx     = w.x + 14;
    const int by     = w.y + 42;
    const int bw     = w.w - 28;
    const int bh     = w.h - 118;
    const int clip_r = w.x + w.w - 14;

    fill_round_rect(bx, by, bw, bh, 6, 0x060D18);
    stroke_round_rect(bx, by, bw, bh, 6, 0x1E3A5F);

    const int start = clamp_i(g_ui.term_scroll, 0, max_i(0, g_ui.term_line_count - kTermVisibleRows));
    for (int r = 0; r < kTermVisibleRows; ++r) {
        const int idx = start + r;
        if (idx >= g_ui.term_line_count) break;
        const uint32_t col = (g_ui.term_lines[idx][0] == '$') ? 0x38BDF8 : 0xD1FAE5;
        draw_text(g_ui.term_lines[idx], bx + 10, by + 10 + r * 17, col, clip_r - 8);
    }

    const int py = by + bh + 8;
    fill_round_rect(bx, py, bw, 28, 5, 0x0A1525);
    stroke_round_rect(bx, py, bw, 28, 5, focused ? 0x22C55E : 0x243B55);

    char prompt_buf[128];
    size_t p = 0;
    p = fmt_str_cap(prompt_buf, sizeof(prompt_buf), p, g_ui.term_cwd);
    p = fmt_str_cap(prompt_buf, sizeof(prompt_buf), p, "> ");
    p = fmt_str_cap(prompt_buf, sizeof(prompt_buf), p, g_ui.term_input);
    fmt_str_cap(prompt_buf, sizeof(prompt_buf), p, "_");
    draw_text(prompt_buf, bx + 8, py + 10, 0xF8FAFC, clip_r - 6);

    const int btn_y = w.y + w.h - 34;
    draw_button(w.x + 14,  btn_y, 92,  24, "Ejecutar",    0x123824, 0x22C55E, 0xECFDF5);
    draw_button(w.x + 114, btn_y, 82,  24, "Limpiar",     0x17253B, 0x38BDF8, 0xF0F9FF);
    draw_button(w.x + 204, btn_y, 76,  24, "Subir",       0x17253B, 0x64748B, 0xE2E8F0);
    draw_button(w.x + 288, btn_y, 76,  24, "Bajar",       0x17253B, 0x64748B, 0xE2E8F0);
    draw_button(w.x + 372, btn_y, 130, 24, "Tema Aurora/Slt", 0x251B3E, 0xA855F7, 0xF3E8FF);
    draw_button(w.x + 510, btn_y, 156, 24, "Guardar fipsos.cfg", 0x1B2A4A, 0xFBBF24, 0xFEF3C7);
}

bool terminal_handle_window_click(const FloatingWindow& w, int mx, int my) noexcept {
    const int btn_y = w.y + w.h - 34;
    if (pt_in_rect(mx, my, w.x + 14, btn_y, 92, 24)) {
        terminal_handle_key('\n');
        return true;
    }
    if (pt_in_rect(mx, my, w.x + 114, btn_y, 82, 24)) {
        terminal_exec_command("clear");
        return true;
    }
    if (pt_in_rect(mx, my, w.x + 204, btn_y, 76, 24)) {
        terminal_scroll(-4);
        return true;
    }
    if (pt_in_rect(mx, my, w.x + 288, btn_y, 76, 24)) {
        terminal_scroll(4);
        return true;
    }
    if (pt_in_rect(mx, my, w.x + 372, btn_y, 130, 24)) {
        toggle_desktop_theme();
        return true;
    }
    if (pt_in_rect(mx, my, w.x + 510, btn_y, 156, 24)) {
        save_desktop_config();
        return true;
    }
    return false;
}

} // namespace fipsos::ui
