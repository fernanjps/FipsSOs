#include "ui_apps.hpp"
#include "ui_wm.hpp"

namespace fipsos::ui {

namespace {

struct AppCenterItem {
    const char* name;
    const char* desc;
    const char* url_path;
    const char* dest_path;
};

constexpr AppCenterItem kAppCenterItems[3] = {
    { "sample_tool.elf",    "Diagnostico del sistema y llamadas POSIX", "/sample_tool.elf",    "/disk/bin/sample_tool.elf" },
    { "benchmark_simd.elf", "Calculo vectorial FP32 SSE2 (128 bits)",   "/benchmark_simd.elf", "/disk/bin/benchmark_simd.elf" },
    { "editor_kilo.elf",    "Editor de texto modal en consola TTY",    "/editor_kilo.elf",    "/disk/bin/editor_kilo.elf" },
};

bool str_starts_with(const char* s, const char* prefix) noexcept {
    while (*prefix != '\0') {
        if (*s != *prefix) return false;
        ++s;
        ++prefix;
    }
    return true;
}

bool match_hwmon_line(const char* s) noexcept {
    return str_starts_with(s, "Topology:") ||
           str_starts_with(s, "Gobernador") ||
           str_starts_with(s, "Memoria Fisica:") ||
           str_starts_with(s, "[NIC]") ||
           str_starts_with(s, "[SATA]") ||
           str_starts_with(s, "[NVMe]") ||
           str_starts_with(s, "[USB]") ||
           str_starts_with(s, "Recuperacion");
}

} // namespace

void push_spark_sample(uint8_t v) noexcept {
    g_ui.sparkline[g_ui.spark_head % 24U] = v;
    g_ui.spark_head += 1U;
}

void refresh_hwmon_file() noexcept {
    const int64_t fd = fipsos::sys_openat(fipsos::AT_FDCWD, "/sys/fipsos/hwmon", fipsos::O_RDONLY, 0);
    if (fd < 0) return;

    char buf[1536];
    const int64_t nread = fipsos::sys_read(static_cast<uint64_t>(fd), buf, sizeof(buf) - 1);
    fipsos::sys_close(static_cast<uint64_t>(fd));
    if (nread <= 0) return;

    const size_t total = static_cast<size_t>(nread);
    buf[total] = '\0';
    g_ui.hwmon_line_count = 0;

    size_t pos = 0;
    while (pos < total && g_ui.hwmon_line_count < CompositorState::kMaxHwmonLines) {
        size_t end = pos;
        while (end < total && buf[end] != '\n') ++end;

        size_t s = pos;
        while (s < end && (buf[s] == ' ' || buf[s] == '-' || buf[s] == '\t')) ++s;

        char tmp[128];
        const size_t l = (end - s < sizeof(tmp) - 1) ? (end - s) : (sizeof(tmp) - 1);
        for (size_t i = 0; i < l; ++i) tmp[i] = buf[s + i];
        tmp[l] = '\0';

        if (match_hwmon_line(tmp)) {
            const int idx = g_ui.hwmon_line_count++;
            set_cstr(g_ui.hwmon_lines[idx], sizeof(g_ui.hwmon_lines[idx]), tmp);
        }
        pos = end + 1;
    }
}

void refresh_kernel_telemetry() noexcept {
    if (g_ui.fb_fd >= 0) {
        fipsos::sys_ioctl(
            static_cast<uint64_t>(g_ui.fb_fd),
            fipsos::VFS_IOCTL_SYS_TELEMETRY,
            reinterpret_cast<uint64_t>(&g_ui.sys_tel)
        );
        fipsos::sys_ioctl(
            static_cast<uint64_t>(g_ui.fb_fd),
            fipsos::VFS_IOCTL_BG_HTTPD_INFO,
            reinterpret_cast<uint64_t>(&g_ui.bg_httpd)
        );
        fipsos::sys_ioctl(
            static_cast<uint64_t>(g_ui.fb_fd),
            fipsos::FIPSOS_IOCTL_LIVE_KERNEL_TELEMETRY,
            reinterpret_cast<uint64_t>(&g_ui.live_tel)
        );
    }
}

void action_ping_gateway(bool log_status) noexcept {
    if (g_ui.net_fd >= 0) {
        const int64_t raw = fipsos::sys_ioctl(
            static_cast<uint64_t>(g_ui.net_fd),
            fipsos::NET_IOCTL_PING_GATEWAY,
            log_status ? 1ULL : 0ULL
        );
        if (raw >= 0) {
            const fipsos::NetPingTelemetry nt = fipsos::unpack_net_telemetry(static_cast<uint64_t>(raw));
            g_ui.last_rtt_us = nt.rtt_us;
            push_spark_sample(static_cast<uint8_t>(clamp_i(14 + static_cast<int>((nt.rtt_us / 20U) & 27U), 8, 42)));
            if (log_status) {
                set_cstr(g_ui.last_action, sizeof(g_ui.last_action), "Ping ARP+ICMP 10.0.2.2 OK");
            }
        }
    }
    refresh_kernel_telemetry();
}

void action_run_npu() noexcept {
    if (g_ui.accel_fd >= 0 && g_ui.model_fd >= 0) {
        const int64_t score = fipsos::sys_ioctl(
            static_cast<uint64_t>(g_ui.accel_fd),
            fipsos::ACCEL_IOCTL_RUN_INFERENCE,
            static_cast<uint64_t>(g_ui.model_fd)
        );
        if (score >= 0) {
            g_ui.npu_energy_milli = static_cast<uint32_t>(score);
            g_ui.npu_runs += 1U;
            push_spark_sample(static_cast<uint8_t>(clamp_i(16 + static_cast<int>((score / 220) % 26), 10, 42)));
            set_cstr(g_ui.last_action, sizeof(g_ui.last_action), "Inferencia C11 NPU FP32 completada");
        }
    }
    refresh_kernel_telemetry();
}

void action_save_disk() noexcept {
    if (g_ui.fb_fd >= 0) {
        const int64_t bytes = fipsos::sys_ioctl(
            static_cast<uint64_t>(g_ui.fb_fd),
            fipsos::DISK_IOCTL_SAVE_NOTE,
            0ULL
        );
        if (bytes > 0) {
            push_spark_sample(36);
            set_cstr(g_ui.last_action, sizeof(g_ui.last_action), "SATA DiskFS: /disk/persistent_note.txt OK");
        }
    }
    refresh_kernel_telemetry();
}

void action_spawn_bg_task() noexcept {
    refresh_kernel_telemetry();
    if (g_ui.bg_httpd.active != 0ULL) {
        set_cstr(g_ui.last_action, sizeof(g_ui.last_action), "Servidor Web :8080 ya activo");
        return;
    }
    const char* const argv[] = { "/bin/httpd", "serve", "8080", "--bg", nullptr };
    const int64_t rc = exec_child_capturing("/bin/httpd", argv, "/tmp/httpd_bg.log", false, true);
    push_spark_sample(36);
    if (rc == 0) {
        set_cstr(g_ui.last_action, sizeof(g_ui.last_action), "Servidor Web HTTP :8080 activo en BG");
    } else {
        set_cstr(g_ui.last_action, sizeof(g_ui.last_action), "Error iniciando httpd :8080");
    }
    refresh_hwmon_file();
    refresh_kernel_telemetry();
}

void action_kill_task() noexcept {
    if (g_ui.bg_httpd.active != 0ULL && g_ui.bg_httpd.pid != 0ULL) {
        if (fipsos::sys_kill(g_ui.bg_httpd.pid, 9ULL) == 0) {
            set_cstr(g_ui.last_action, sizeof(g_ui.last_action), "Servidor Web :8080 detenido");
        } else {
            set_cstr(g_ui.last_action, sizeof(g_ui.last_action), "Error deteniendo Web :8080");
        }
        push_spark_sample(20);
    } else if (g_ui.fb_fd >= 0) {
        const uint64_t cmd = (g_ui.sys_tel.bg_task_state == 1) ? 2ULL : 3ULL;
        fipsos::sys_ioctl(static_cast<uint64_t>(g_ui.fb_fd), fipsos::VFS_IOCTL_TASK_CONTROL, cmd);
        set_cstr(g_ui.last_action, sizeof(g_ui.last_action),
                 (cmd == 2ULL) ? "bg_downloader terminado" : "simd-worker terminado");
        push_spark_sample(20);
    }
    refresh_hwmon_file();
    refresh_kernel_telemetry();
}

void action_refresh_net() noexcept {
    action_ping_gateway(false);
    refresh_hwmon_file();
    push_spark_sample(28);
    set_cstr(g_ui.last_action, sizeof(g_ui.last_action), "Hwmon + Red eth0 actualizados");
}

void action_appcenter_cycle() noexcept {
    g_ui.appcenter_selected = (g_ui.appcenter_selected + 1) % 3;
    size_t cp = 0;
    cp = fmt_str_cap(g_ui.appcenter_console, sizeof(g_ui.appcenter_console), cp, "Seleccionado: ");
    fmt_str_cap(g_ui.appcenter_console, sizeof(g_ui.appcenter_console), cp, kAppCenterItems[g_ui.appcenter_selected].name);
    refresh_kernel_telemetry();
}

void action_appcenter_install() noexcept {
    const int idx = g_ui.appcenter_selected;
    if (idx < 0 || idx >= 3) return;
    const AppCenterItem& app = kAppCenterItems[idx];

    const int64_t sock_fd = fipsos::sys_socket(fipsos::AF_INET, fipsos::SOCK_STREAM, 0);
    if (sock_fd < 0) {
        set_cstr(g_ui.appcenter_console, sizeof(g_ui.appcenter_console), "Error: socket() fallo");
        return;
    }

    fipsos::SockAddrIn server_addr{};
    server_addr.sin_family = fipsos::AF_INET;
    server_addr.sin_port = static_cast<uint16_t>(((8080U & 0xFFU) << 8) | ((8080U >> 8) & 0xFFU));
    const uint8_t ip[4] = {10, 0, 2, 2};
    for (size_t i = 0; i < 4; ++i) {
        reinterpret_cast<uint8_t*>(&server_addr.sin_addr.s_addr)[i] = ip[i];
    }

    if (fipsos::sys_connect(sock_fd, &server_addr, sizeof(server_addr)) < 0) {
        fipsos::sys_close(static_cast<uint64_t>(sock_fd));
        set_cstr(g_ui.appcenter_console, sizeof(g_ui.appcenter_console), "Error: Host 10.0.2.2:8080 inalcanzable");
        return;
    }

    char req_buf[256];
    size_t req_len = 0;
    req_len = fmt_str_cap(req_buf, sizeof(req_buf), req_len, "GET ");
    req_len = fmt_str_cap(req_buf, sizeof(req_buf), req_len, app.url_path);
    req_len = fmt_str_cap(req_buf, sizeof(req_buf), req_len, " HTTP/1.0\r\nHost: 10.0.2.2:8080\r\nConnection: close\r\n\r\n");
    fipsos::sys_sendto(sock_fd, req_buf, req_len, 0);

    char recv_buf[1024];
    size_t hdr_len = 0;
    size_t body_off = 0;
    bool found_hdr_end = false;
    while (hdr_len < sizeof(recv_buf)) {
        const int64_t n = fipsos::sys_recvfrom(sock_fd, recv_buf + hdr_len, sizeof(recv_buf) - hdr_len, 0);
        if (n <= 0) {
            break;
        }
        hdr_len += static_cast<size_t>(n);
        for (size_t i = 0; i + 3 < hdr_len; ++i) {
            if (recv_buf[i] == '\r' && recv_buf[i + 1] == '\n' && recv_buf[i + 2] == '\r' && recv_buf[i + 3] == '\n') {
                body_off = i + 4;
                found_hdr_end = true;
                break;
            }
        }
        if (found_hdr_end) {
            break;
        }
    }
    if (hdr_len == 0) {
        fipsos::sys_close(static_cast<uint64_t>(sock_fd));
        set_cstr(g_ui.appcenter_console, sizeof(g_ui.appcenter_console), "Error: Respuesta HTTP vacia");
        return;
    }

    // Verify strict HTTP 200 status (" 200 " or " 200\r"/"\n") and body bounds before truncating dest_path
    bool status_ok = false;
    for (size_t i = 0; i + 4 < hdr_len && i < 32; ++i) {
        if (recv_buf[i] == ' ' && recv_buf[i + 1] == '2' && recv_buf[i + 2] == '0' && recv_buf[i + 3] == '0') {
            const char after = recv_buf[i + 4];
            if (after == ' ' || after == '\r' || after == '\n') {
                status_ok = true;
                break;
            }
        }
        if (recv_buf[i] == '\r' || recv_buf[i] == '\n') {
            break;
        }
    }

    if (!status_ok || !found_hdr_end || body_off > hdr_len) {
        fipsos::sys_close(static_cast<uint64_t>(sock_fd));
        set_cstr(g_ui.appcenter_console, sizeof(g_ui.appcenter_console), "Error: HTTP != 200 o cabecera invalida");
        return;
    }

    const int64_t disk_fd = fipsos::sys_open(app.dest_path, fipsos::O_CREAT | fipsos::O_TRUNC | fipsos::O_RDWR);
    if (disk_fd < 0) {
        fipsos::sys_close(static_cast<uint64_t>(sock_fd));
        set_cstr(g_ui.appcenter_console, sizeof(g_ui.appcenter_console), "Error: No se pudo crear en /disk/bin/");
        return;
    }

    size_t total_written = 0;
    const size_t body_len = hdr_len - body_off;
    if (body_len > 0) {
        fipsos::sys_write(static_cast<uint64_t>(disk_fd), recv_buf + body_off, body_len);
        total_written += body_len;
    }
    int64_t n = 0;
    while ((n = fipsos::sys_recvfrom(sock_fd, recv_buf, sizeof(recv_buf), 0)) > 0) {
        fipsos::sys_write(static_cast<uint64_t>(disk_fd), recv_buf, static_cast<size_t>(n));
        total_written += static_cast<size_t>(n);
    }

    fipsos::sys_close(static_cast<uint64_t>(disk_fd));
    fipsos::sys_close(static_cast<uint64_t>(sock_fd));
    g_ui.appcenter_installed[idx] = true;

    size_t cp = 0;
    cp = fmt_str_cap(g_ui.appcenter_console, sizeof(g_ui.appcenter_console), cp, "[TCP OK] ");
    cp = fmt_str_cap(g_ui.appcenter_console, sizeof(g_ui.appcenter_console), cp, app.name);
    cp = fmt_str_cap(g_ui.appcenter_console, sizeof(g_ui.appcenter_console), cp, " (");
    cp = fmt_u32(g_ui.appcenter_console, cp, static_cast<uint32_t>(total_written));
    fmt_str_cap(g_ui.appcenter_console, sizeof(g_ui.appcenter_console), cp, " B)");
    refresh_kernel_telemetry();
}

void action_appcenter_launch() noexcept {
    const int idx = g_ui.appcenter_selected;
    if (idx < 0 || idx >= 3) return;
    const AppCenterItem& app = kAppCenterItems[idx];
    set_cstr(g_ui.appcenter_last_exec, sizeof(g_ui.appcenter_last_exec), app.name);
    action_launch_elf(app.dest_path, app.name);
    size_t cp = 0;
    cp = fmt_str_cap(g_ui.appcenter_console, sizeof(g_ui.appcenter_console), cp, "[Ejecutado] ");
    fmt_str_cap(g_ui.appcenter_console, sizeof(g_ui.appcenter_console), cp, app.name);
}

void action_launch_elf(const char* elf_path, const char* label) noexcept {
    g_ui.start_open = false;
    compose_all_to_backbuf();
    flip_full();

    const int64_t rc = exec_child_capturing(elf_path, nullptr, nullptr, false, true);
    g_ui.app_launches += 1U;
    push_spark_sample(38);
    refresh_kernel_telemetry();

    size_t p = 0;
    p = fmt_str(g_ui.fmt, p, label);
    p = fmt_str(g_ui.fmt, p, " -> exit(");
    p = fmt_u32(g_ui.fmt, p, (rc >= 0) ? static_cast<uint32_t>(rc) : 99U);
    p = fmt_str(g_ui.fmt, p, ") OK");
    set_cstr(g_ui.last_action, sizeof(g_ui.last_action), g_ui.fmt);

    compose_all_to_backbuf();
    flip_full();
}

void draw_win_appcenter(const FloatingWindow& w, bool focused) noexcept {
    draw_window_chrome(w, focused);
    const int cx     = w.x + 14;
    const int cy     = w.y + 40;
    const int box_w  = w.w - 28;
    const int clip_r = cx + box_w - 8;

    fill_round_rect(cx, cy, box_w, 24, 4, 0x0F1E36);
    stroke_round_rect(cx, cy, box_w, 24, 4, 0x2563EB);
    draw_text("Repositorio FipSOs Host: http://10.0.2.2:8080/ [TCP Sockets]", cx + 8, cy + 8, 0x60A5FA, clip_r);

    for (int i = 0; i < 3; ++i) {
        const int card_y = cy + 28 + i * 42;
        const bool is_sel = (i == g_ui.appcenter_selected);
        fill_round_rect(cx, card_y, box_w, 38, 5, is_sel ? 0x172554 : 0x0B1324);
        stroke_round_rect(cx, card_y, box_w, 38, 5, is_sel ? 0xFBBF24 : 0x1E3A5F);
        if (is_sel) fill_round_rect(cx + 6, card_y + 11, 5, 16, 2, 0xFBBF24);

        const int badge_x = cx + box_w - 110;
        const int text_r  = badge_x - 8;
        draw_text(kAppCenterItems[i].name, cx + 16, card_y + 6, is_sel ? 0xFBBF24 : 0x38BDF8, text_r);
        draw_text(kAppCenterItems[i].desc, cx + 16, card_y + 22, 0x94A3B8, text_r);

        const bool inst = g_ui.appcenter_installed[i];
        fill_round_rect(badge_x, card_y + 8, 100, 22, 4, inst ? 0x064E3B : 0x451A03);
        stroke_round_rect(badge_x, card_y + 8, 100, 22, 4, inst ? 0x34D399 : 0xF59E0B);
        draw_text_centered(inst ? "INSTALADO" : "DISPONIBLE", badge_x, card_y + 8, 100, 22, inst ? 0x34D399 : 0xFBBF24);
    }

    const int inf_y = cy + 158;
    fill_round_rect(cx, inf_y, w.w - 28, 38, 5, 0x091220);
    stroke_round_rect(cx, inf_y, w.w - 28, 38, 5, 0x1E3A5F);
    size_t p = 0;
    p = fmt_str(g_ui.fmt, p, "SATA /disk/bin/ (");
    p = fmt_u32(g_ui.fmt, p, static_cast<uint32_t>(g_ui.sys_tel.disk_files_count));
    p = fmt_str(g_ui.fmt, p, " arch) | Ultima: ");
    p = fmt_str(g_ui.fmt, p, g_ui.appcenter_last_exec);
    draw_text(g_ui.fmt, cx + 8, inf_y + 6, 0xE2E8F0, clip_r);

    p = 0;
    p = fmt_str(g_ui.fmt, p, "CR3: ");
    p = fmt_hex_u64(g_ui.fmt, p, g_ui.sys_tel.active_cr3);
    p = fmt_str(g_ui.fmt, p, " | SMP APIC: 2 Cores | ACPI HLT: ACTIVO");
    draw_text(g_ui.fmt, cx + 8, inf_y + 22, 0x38BDF8, clip_r);

    const int con_y = inf_y + 42;
    fill_round_rect(cx, con_y, w.w - 28, 34, 5, 0x080E1A);
    stroke_round_rect(cx, con_y, w.w - 28, 34, 5, 0x334155);
    draw_text("Consola:", cx + 8, con_y + 12, 0x38BDF8, clip_r);
    draw_text(g_ui.appcenter_console, cx + 76, con_y + 12, 0xF8FAFC, clip_r);

    const int by = w.y + w.h - 36;
    draw_button(w.x + 14,  by, 160, 26, "Instalar via TCP", 0x1E3A8A, 0x38BDF8, 0xF8FAFC);
    draw_button(w.x + 184, by, 140, 26, "Lanzar App",       0x064E3B, 0x34D399, 0xF8FAFC);
    draw_button(w.x + 334, by, 130, 26, "Ciclar App",       0x4C1D95, 0xC084FC, 0xF8FAFC);
    draw_button(w.x + 474, by, 112, 26, "Guardar Dsk",      0x1B2A4A, 0xFBBF24, 0xF8FAFC);
}

bool apps_handle_window_click(int wid, const FloatingWindow& w, int mx, int my) noexcept {
    const int by = w.y + w.h - 36;
    if (wid == 3) {
        const int cx = w.x + 14;
        const int cy = w.y + 40;
        for (int i = 0; i < 3; ++i) {
            if (pt_in_rect(mx, my, cx, cy + 28 + i * 42, w.w - 28, 38)) {
                g_ui.appcenter_selected = i;
                return true;
            }
        }
        if (pt_in_rect(mx, my, w.x + 14,  by, 160, 26)) { action_appcenter_install(); return true; }
        if (pt_in_rect(mx, my, w.x + 184, by, 140, 26)) { action_appcenter_launch();  return true; }
        if (pt_in_rect(mx, my, w.x + 334, by, 130, 26)) { action_appcenter_cycle();   return true; }
        if (pt_in_rect(mx, my, w.x + 474, by, 112, 26)) { action_save_disk();         return true; }
    }
    return false;
}

namespace {

bool cfg_starts_with(const char* s, const char* pfx) noexcept {
    while (*pfx) {
        if (*s++ != *pfx++) return false;
    }
    return true;
}

void cfg_print_cstr(const char* s) noexcept {
    fipsos::sys_write(fipsos::FD_STDOUT, s, str_len(s));
}

} // namespace

void load_desktop_config() noexcept {
    g_ui.theme_id = 0U;
    set_cstr(g_ui.cfg_status, sizeof(g_ui.cfg_status), "Tema: Aurora (defecto)");
    const int64_t fd = fipsos::sys_openat(fipsos::AT_FDCWD, "/mnt/fat0/fipsos.cfg", fipsos::O_RDONLY, 0);
    if (fd < 0) return;

    char buf[256];
    const int64_t n = fipsos::sys_read(static_cast<uint64_t>(fd), buf, sizeof(buf) - 1);
    fipsos::sys_close(static_cast<uint64_t>(fd));
    if (n <= 0) return;
    buf[n] = '\0';

    size_t pos = 0;
    while (pos < static_cast<size_t>(n)) {
        char line[96];
        size_t lp = 0;
        while (pos < static_cast<size_t>(n) && buf[pos] != '\n' && buf[pos] != '\r') {
            if (lp + 1 < sizeof(line)) line[lp++] = buf[pos];
            ++pos;
        }
        line[lp] = '\0';
        while (pos < static_cast<size_t>(n) && (buf[pos] == '\n' || buf[pos] == '\r')) ++pos;

        if (str_eq(line, "theme=slate")) {
            g_ui.theme_id = 1U;
            set_cstr(g_ui.cfg_status, sizeof(g_ui.cfg_status), "Tema: Dark Slate (cfg)");
        } else if (str_eq(line, "theme=aurora")) {
            g_ui.theme_id = 0U;
            set_cstr(g_ui.cfg_status, sizeof(g_ui.cfg_status), "Tema: Aurora (cfg)");
        } else if (cfg_starts_with(line, "exp_dir=") && line[8] == '/') {
            set_cstr(g_ui.exp_dir, sizeof(g_ui.exp_dir), line + 8);
        }
    }
    fipsos::print_literal("[desktop_ui] cfg_loaded theme=");
    cfg_print_cstr(g_ui.theme_id == 1U ? "slate" : "aurora");
    fipsos::print_literal(" exp_dir=");
    cfg_print_cstr(g_ui.exp_dir);
    fipsos::print_literal("\n");
}

void save_desktop_config() noexcept {
    const int64_t fd = fipsos::sys_openat(
        fipsos::AT_FDCWD,
        "/mnt/fat0/fipsos.cfg",
        fipsos::O_CREAT | fipsos::O_TRUNC | fipsos::O_RDWR,
        0644
    );
    if (fd < 0) {
        set_cstr(g_ui.cfg_status, sizeof(g_ui.cfg_status), "Error guardando fipsos.cfg");
        return;
    }
    char buf[160];
    size_t p = 0;
    p = fmt_str_cap(buf, sizeof(buf), p, "theme=");
    p = fmt_str_cap(buf, sizeof(buf), p, (g_ui.theme_id == 1U) ? "slate\n" : "aurora\n");
    p = fmt_str_cap(buf, sizeof(buf), p, "exp_dir=");
    p = fmt_str_cap(buf, sizeof(buf), p, g_ui.exp_dir);
    p = fmt_str_cap(buf, sizeof(buf), p, "\n");
    fipsos::sys_write(static_cast<uint64_t>(fd), buf, p);
    fipsos::sys_close(static_cast<uint64_t>(fd));

    set_cstr(g_ui.cfg_status, sizeof(g_ui.cfg_status), "Cfg guardada /mnt/fat0");
    set_cstr(g_ui.last_action, sizeof(g_ui.last_action), "Preferencias guardadas en fipsos.cfg");
    fipsos::print_literal("[desktop_ui] cfg_saved theme=");
    cfg_print_cstr(g_ui.theme_id == 1U ? "slate" : "aurora");
    fipsos::print_literal(" exp_dir=");
    cfg_print_cstr(g_ui.exp_dir);
    fipsos::print_literal("\n");
}

void toggle_desktop_theme() noexcept {
    g_ui.theme_id ^= 1U;
    init_wallpaper_luts();
    set_cstr(g_ui.cfg_status, sizeof(g_ui.cfg_status),
             (g_ui.theme_id == 1U) ? "Tema: Dark Slate" : "Tema: Aurora");
    set_cstr(g_ui.last_action, sizeof(g_ui.last_action),
             (g_ui.theme_id == 1U) ? "Tema cambiado a Dark Slate" : "Tema cambiado a Aurora");
    fipsos::print_literal("[desktop_ui] theme=");
    cfg_print_cstr(g_ui.theme_id == 1U ? "slate" : "aurora");
    fipsos::print_literal("\n");
}

} // namespace fipsos::ui
