#include "ui_security.hpp"
#include "ui_wm.hpp"
#include "simd_nn.hpp"

namespace fipsos::ui {

namespace {

struct SecurityPanelState {
    char verdict[16];
    char backend[12];
    char blocks[12];
    char log_lines[7][68];
    int  log_count;
    char status[68];
    bool loaded;
};

SecurityPanelState s_sec{};

void parse_guard_file() noexcept {
    set_cstr(s_sec.verdict, sizeof(s_sec.verdict), "SAFE");
    set_cstr(s_sec.backend, sizeof(s_sec.backend), fipsos::nn::simd_backend_name());
    set_cstr(s_sec.blocks, sizeof(s_sec.blocks), "0");

    const int64_t fd = fipsos::sys_open("/mnt/fat0/ai_guard.state", fipsos::O_RDONLY);
    if (fd < 0) return;
    char raw[64]{};
    const int64_t n = fipsos::sys_read(static_cast<uint64_t>(fd), raw, sizeof(raw) - 1);
    fipsos::sys_close(static_cast<uint64_t>(fd));
    if (n <= 3) return;

    int part = 0, pi = 0;
    for (int i = 0; i < n && raw[i] != '\0' && raw[i] != '\n' && raw[i] != '\r'; ++i) {
        if (raw[i] == '|') { ++part; pi = 0; continue; }
        if (part == 0 && pi < 15) { s_sec.verdict[pi++] = raw[i]; s_sec.verdict[pi] = '\0'; }
        else if (part == 1 && pi < 11) { s_sec.backend[pi++] = raw[i]; s_sec.backend[pi] = '\0'; }
        else if (part == 2 && pi < 11) { s_sec.blocks[pi++] = raw[i]; s_sec.blocks[pi] = '\0'; }
    }
}

void load_security_log() noexcept {
    s_sec.log_count = 0;
    const int64_t fd = fipsos::sys_open("/mnt/fat0/security_ai.log", fipsos::O_RDONLY);
    if (fd < 0) {
        set_cstr(s_sec.log_lines[0], 68, "(Sin incidentes en /mnt/fat0/security_ai.log)");
        s_sec.log_count = 1;
        return;
    }
    static char buf[2048];
    const int64_t n = fipsos::sys_read(static_cast<uint64_t>(fd), buf, sizeof(buf) - 1);
    fipsos::sys_close(static_cast<uint64_t>(fd));
    if (n <= 0) {
        set_cstr(s_sec.log_lines[0], 68, "(Registro /mnt/fat0/security_ai.log vacio)");
        s_sec.log_count = 1;
        return;
    }
    buf[n] = '\0';
    int start = 0;
    for (int i = 0; i <= n; ++i) {
        if (buf[i] == '\n' || buf[i] == '\r' || buf[i] == '\0') {
            if (i > start) {
                const int slot = s_sec.log_count % 7;
                int j = 0;
                for (int k = start; k < i && j < 67; ++k) {
                    const uint8_t c = static_cast<uint8_t>(buf[k]);
                    s_sec.log_lines[slot][j++] = (c >= 0x20U && c <= 0x7EU) ? static_cast<char>(c) : '.';
                }
                s_sec.log_lines[slot][j] = '\0';
                ++s_sec.log_count;
            }
            start = i + 1;
        }
    }
    if (s_sec.log_count > 7) {
        char ordered[7][68]{};
        const int base = s_sec.log_count - 7;
        for (int r = 0; r < 7; ++r) {
            set_cstr(ordered[r], 68, s_sec.log_lines[(base + r) % 7]);
        }
        for (int r = 0; r < 7; ++r) set_cstr(s_sec.log_lines[r], 68, ordered[r]);
        s_sec.log_count = 7;
    }
    if (s_sec.log_count == 0) {
        set_cstr(s_sec.log_lines[0], 68, "(Sin registros de seguridad)");
        s_sec.log_count = 1;
    }
}

void ensure_sec_defaults() noexcept {
    if (s_sec.verdict[0] == '\0') set_cstr(s_sec.verdict, sizeof(s_sec.verdict), "SAFE");
    if (s_sec.backend[0] == '\0') set_cstr(s_sec.backend, sizeof(s_sec.backend), fipsos::nn::simd_backend_name());
    if (s_sec.blocks[0] == '\0') set_cstr(s_sec.blocks, sizeof(s_sec.blocks), "0");
    if (s_sec.status[0] == '\0') set_cstr(s_sec.status, sizeof(s_sec.status), "Guardian Neuronal listo (clic Auditar / Guard IA)");
    if (s_sec.log_count <= 0) {
        set_cstr(s_sec.log_lines[0], 68, "(Estado en RAM listo - pulsa Auditar Disco o Probar Guard IA)");
        s_sec.log_count = 1;
    }
}

void write_fat_text(const char* path, const char* text) noexcept {
    const int64_t fd = fipsos::sys_openat(fipsos::AT_FDCWD, path, fipsos::O_CREAT | fipsos::O_RDWR | fipsos::O_TRUNC, 0644);
    if (fd >= 0) {
        fipsos::sys_write(static_cast<uint64_t>(fd), text, str_len(text));
        fipsos::sys_close(static_cast<uint64_t>(fd));
    }
}

void action_audit_disk() noexcept {
    const char* argv[] = {"/bin/goose_audit", nullptr};
    const int64_t rc = exec_child_capturing("/bin/goose_audit", argv, "/tmp/sec_audit.log", false);
    const int64_t fd = fipsos::sys_open("/tmp/sec_audit.log", fipsos::O_RDONLY);
    if (fd >= 0) {
        char buf[128]{};
        const int64_t n = fipsos::sys_read(static_cast<uint64_t>(fd), buf, sizeof(buf) - 1);
        fipsos::sys_close(static_cast<uint64_t>(fd));
        if (n > 0) {
            for (int i = 0; i < n; ++i) {
                if (buf[i] == '\n' || buf[i] == '\r') { buf[i] = '\0'; break; }
            }
            set_cstr(s_sec.status, sizeof(s_sec.status), buf);
            return;
        }
    }
    set_cstr(s_sec.status, sizeof(s_sec.status), (rc == 0) ? "Auditoria /bin/goose_audit completada" : "Error en goose_audit");
}

void action_test_guard() noexcept {
    const char* argv[] = {"/bin/fips_ai", "--guard", "35", "2", "5", "6", "0", "25", nullptr};
    exec_child_capturing("/bin/fips_ai", argv, "/tmp/sec_guard.log", false);
    char st[48]{};
    size_t p = fmt_str_cap(st, sizeof(st), 0, "INTRUSION|");
    p = fmt_str_cap(st, sizeof(st), p, fipsos::nn::simd_backend_name());
    fmt_str_cap(st, sizeof(st), p, "|1\n");
    write_fat_text("/mnt/fat0/ai_guard.state", st);
    security_refresh_state();
    set_cstr(s_sec.status, sizeof(s_sec.status), "Simulacion --guard: INTRUSION detectada y sandbox aplicado");
}

void action_test_audio() noexcept {
    const int64_t fd = fipsos::sys_open("/dev/audio", fipsos::O_RDWR);
    if (fd >= 0) {
        const char tone[] = "523:60";
        fipsos::sys_write(static_cast<uint64_t>(fd), tone, sizeof(tone) - 1);
        fipsos::sys_close(static_cast<uint64_t>(fd));
        set_cstr(s_sec.status, sizeof(s_sec.status), "Tono 523Hz (60ms) emitido en /dev/audio");
    } else {
        set_cstr(s_sec.status, sizeof(s_sec.status), "No se pudo abrir /dev/audio");
    }
}

void action_clear_alerts() noexcept {
    write_fat_text("/mnt/fat0/security_ai.log", "");
    char st[48]{};
    size_t p = fmt_str_cap(st, sizeof(st), 0, "SAFE|");
    p = fmt_str_cap(st, sizeof(st), p, fipsos::nn::simd_backend_name());
    fmt_str_cap(st, sizeof(st), p, "|0\n");
    write_fat_text("/mnt/fat0/ai_guard.state", st);
    security_refresh_state();
    set_cstr(s_sec.status, sizeof(s_sec.status), "Alertas limpiadas -> Estado SAFE");
}

} // namespace

void security_refresh_state() noexcept {
    parse_guard_file();
    load_security_log();
    if (!s_sec.loaded) {
        set_cstr(s_sec.status, sizeof(s_sec.status), "Guardian Neuronal en linea (AVX2/SSE2 + /dev/audio)");
        s_sec.loaded = true;
    }
}

void draw_win_security(const FloatingWindow& w, bool focused) noexcept {
    if (!s_sec.loaded) ensure_sec_defaults();
    draw_window_chrome(w, focused);

    const int cx = w.x + 14;
    const int cy = w.y + 40;
    const int cw = w.w - 28;
    const int clip_r = cx + cw - 8;

    const bool is_intr = (s_sec.verdict[0] == 'I');
    const bool is_susp = (s_sec.verdict[0] == 'S' && s_sec.verdict[1] == 'U');
    const uint32_t vcol = is_intr ? 0xEF4444 : is_susp ? 0xFBBF24 : 0x10B981;
    const uint32_t vbg  = is_intr ? 0x3B0D16 : is_susp ? 0x3B2809 : 0x063729;

    fill_round_rect(cx, cy, cw, 62, 6, 0x091220);
    stroke_round_rect(cx, cy, cw, 62, 6, 0x1E3A5F);

    fill_round_rect(cx + 10, cy + 10, 126, 42, 6, vbg);
    stroke_round_rect(cx + 10, cy + 10, 126, 42, 6, vcol);
    draw_text_centered(s_sec.verdict, cx + 10, cy + 23, 126, 16, vcol);

    size_t p = fmt_str_cap(g_ui.fmt, sizeof(g_ui.fmt), 0, "Backend SIMD: ");
    p = fmt_str_cap(g_ui.fmt, sizeof(g_ui.fmt), p, s_sec.backend);
    p = fmt_str_cap(g_ui.fmt, sizeof(g_ui.fmt), p, (s_sec.backend[0] == 'A') ? " (256-bit)" : " (128-bit)");
    draw_text(g_ui.fmt, cx + 148, cy + 12, 0x38BDF8, clip_r);

    p = fmt_str_cap(g_ui.fmt, sizeof(g_ui.fmt), 0, "Bloqueos Autonomos: ");
    p = fmt_str_cap(g_ui.fmt, sizeof(g_ui.fmt), p, s_sec.blocks);
    p = fmt_str_cap(g_ui.fmt, sizeof(g_ui.fmt), p, " | PID: ");
    fmt_u32(g_ui.fmt, p, static_cast<uint32_t>(g_ui.pid), sizeof(g_ui.fmt));
    draw_text(g_ui.fmt, cx + 148, cy + 32, 0xE2E8F0, clip_r);

    const int ly = cy + 70;
    const int lh = w.h - 190;
    fill_round_rect(cx, ly, cw, lh, 6, 0x07111D);
    stroke_round_rect(cx, ly, cw, lh, 6, 0x1E3A5F);
    draw_text("Registros /mnt/fat0/security_ai.log:", cx + 10, ly + 8, 0xFBBF24, clip_r);
    for (int i = 0; i < s_sec.log_count && i < 6; ++i) {
        const uint32_t lc = (s_sec.log_lines[i][0] == '[') ? 0xFCA5A5 : 0x94A3B8;
        draw_text(s_sec.log_lines[i], cx + 10, ly + 28 + i * 20, lc, clip_r);
    }

    const int sy = ly + lh + 6;
    fill_round_rect(cx, sy, cw, 24, 4, 0x081120);
    stroke_round_rect(cx, sy, cw, 24, 4, 0x26466D);
    draw_text(s_sec.status, cx + 10, sy + 8, 0x00F5D4, clip_r);

    const int by = w.y + w.h - 36;
    draw_button(w.x + 14,  by, 138, 26, "Auditar Disco",   0x064E3B, 0x34D399, 0xF8FAFC);
    draw_button(w.x + 160, by, 152, 26, "Probar Guard IA", 0x4A1520, 0xEF4444, 0xF8FAFC);
    draw_button(w.x + 320, by, 128, 26, "Tono Audio",      0x1E3A8A, 0x60A5FA, 0xF8FAFC);
    draw_button(w.x + 456, by, 148, 26, "Limpiar Alertas", 0x311042, 0xC084FC, 0xF8FAFC);
}

bool security_handle_window_click(const FloatingWindow& w, int mx, int my) noexcept {
    const int by = w.y + w.h - 36;
    if (pt_in_rect(mx, my, w.x + 14,  by, 138, 26)) { action_audit_disk();   return true; }
    if (pt_in_rect(mx, my, w.x + 160, by, 152, 26)) { action_test_guard();   return true; }
    if (pt_in_rect(mx, my, w.x + 320, by, 128, 26)) { action_test_audio();   return true; }
    if (pt_in_rect(mx, my, w.x + 456, by, 148, 26)) { action_clear_alerts(); return true; }
    return false;
}

} // namespace fipsos::ui
