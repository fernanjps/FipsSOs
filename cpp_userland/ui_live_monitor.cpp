#include "ui_live_monitor.hpp"
#include "ui_apps.hpp"
#include "ui_wm.hpp"

namespace fipsos::ui {

namespace {

uint32_t g_live_launch_seq = 0U;

size_t fmt_u64_cap(char* buf, size_t cap, size_t pos, uint64_t val) noexcept {
    if (cap == 0) return pos;
    char tmp[24];
    size_t n = 0;
    if (val == 0ULL) {
        tmp[n++] = '0';
    } else {
        while (val > 0ULL && n < sizeof(tmp)) {
            tmp[n++] = static_cast<char>('0' + (val % 10ULL));
            val /= 10ULL;
        }
    }
    while (n > 0 && pos + 1 < cap) {
        buf[pos++] = tmp[--n];
    }
    if (pos < cap) buf[pos] = '\0';
    return pos;
}

const char* proc_state_label(uint32_t st) noexcept {
    switch (st) {
        case 1U: return "RDY";
        case 2U: return "RUN";
        case 3U: return "BLK";
        case 4U: return "ZMB";
        case 5U: return "EXT";
        default: return "---";
    }
}

uint32_t proc_state_color(uint32_t st) noexcept {
    switch (st) {
        case 2U: return 0x10B981;
        case 1U: return 0x38BDF8;
        case 3U: return 0xFBBF24;
        case 4U: return 0xF87171;
        case 5U: return 0x94A3B8;
        default: return 0x64748B;
    }
}

void draw_fixed_proc_name(const char* raw, int x, int y, uint32_t fg, int max_x) noexcept {
    char clean[14];
    int n = 0;
    while (n < 13 && raw[n] != '\0') {
        clean[n] = raw[n];
        ++n;
    }
    if (n == 0) {
        set_cstr(clean, sizeof(clean), "(kernel)");
    } else {
        clean[n] = '\0';
    }
    draw_text(clean, x, y, fg, max_x);
}

} // namespace

void refresh_live_kernel_telemetry() noexcept {
    if (g_ui.fb_fd < 0) return;
    fipsos::sys_ioctl(
        static_cast<uint64_t>(g_ui.fb_fd),
        fipsos::FIPSOS_IOCTL_LIVE_KERNEL_TELEMETRY,
        reinterpret_cast<uint64_t>(&g_ui.live_tel)
    );
}

void live_monitor_run_ram_probe(uint64_t num_pages) noexcept {
    if (g_ui.fb_fd < 0) return;
    fipsos::RamProbeRequest req{};
    req.num_pages = (num_pages == 0ULL) ? 256ULL : num_pages;

    const int64_t rc = fipsos::sys_ioctl(
        static_cast<uint64_t>(g_ui.fb_fd),
        fipsos::FIPSOS_IOCTL_RAM_RW_PROBE,
        reinterpret_cast<uint64_t>(&req)
    );
    refresh_live_kernel_telemetry();
    refresh_kernel_telemetry();

    if (rc >= 0) {
        g_ui.live_probe_tsc = req.tsc_cycles;
        push_spark_sample(34U);
        size_t p = 0;
        p = fmt_str_cap(g_ui.last_action, sizeof(g_ui.last_action), p, "RAM R/W OK: ");
        p = fmt_u32(g_ui.last_action, p, req.pages_ok, sizeof(g_ui.last_action));
        p = fmt_str_cap(g_ui.last_action, sizeof(g_ui.last_action), p, "p @ ");
        p = fmt_u32(g_ui.last_action, p, req.mbps, sizeof(g_ui.last_action));
        fmt_str_cap(g_ui.last_action, sizeof(g_ui.last_action), p, " MiB/s");
    } else {
        set_cstr(g_ui.last_action, sizeof(g_ui.last_action), "Error ejecutando prueba RAM R/W");
    }
}

void live_monitor_launch_process() noexcept {
    const uint32_t seq = g_live_launch_seq++;
    const char* prog = ((seq & 1U) == 0U) ? "/bin/npu_infer.elf" : "/bin/c_std_app.elf";
    const char* label = ((seq & 1U) == 0U) ? "npu_infer" : "c_std_app";

    const int64_t rc = exec_child_capturing(prog, nullptr, nullptr, false, true);
    g_ui.app_launches += 1U;
    push_spark_sample(38U);
    refresh_kernel_telemetry();
    refresh_live_kernel_telemetry();

    size_t p = 0;
    p = fmt_str_cap(g_ui.last_action, sizeof(g_ui.last_action), p, label);
    p = fmt_str_cap(g_ui.last_action, sizeof(g_ui.last_action), p, " #");
    p = fmt_u32(g_ui.last_action, p, seq + 1U, sizeof(g_ui.last_action));
    p = fmt_str_cap(g_ui.last_action, sizeof(g_ui.last_action), p, " rc=");
    p = fmt_u32(g_ui.last_action, p, (rc >= 0) ? static_cast<uint32_t>(rc) : 99U, sizeof(g_ui.last_action));
    p = fmt_str_cap(g_ui.last_action, sizeof(g_ui.last_action), p, " F:");
    fmt_u64_cap(g_ui.last_action, sizeof(g_ui.last_action), p, g_ui.live_tel.free_frames);
}

void draw_win_live_monitor(const FloatingWindow& w, bool focused) noexcept {
    draw_window_chrome(w, focused);
    const int cx = w.x + 14;
    const int cy = w.y + 42;
    const int box_w = w.w - 28;
    const int clip_r = cx + box_w - 8;
    const fipsos::FipsosLiveTelemetry& lt = g_ui.live_tel;

    // 1. Nucleos SMP en Vivo
    fill_round_rect(cx, cy, box_w, 94, 6, 0x091220);
    stroke_round_rect(cx, cy, box_w, 94, 6, 0x1E3A5F);
    {
        size_t p = 0;
        p = fmt_str(g_ui.fmt, p, "Nucleos SMP en Vivo (Online=");
        p = fmt_u32(g_ui.fmt, p, lt.online_cores);
        p = fmt_str(g_ui.fmt, p, " | IRQ0=");
        p = fmt_u64_cap(g_ui.fmt, sizeof(g_ui.fmt), p, lt.irq0_ticks);
        p = fmt_str(g_ui.fmt, p, "t)");
        draw_text(g_ui.fmt, cx + 10, cy + 7, 0x38BDF8, clip_r);
    }

    const int shown_cores = min_i(4, max_i(2, static_cast<int>(lt.online_cores)));
    const int core_step = (shown_cores <= 2) ? 28 : 17;
    for (int ci = 0; ci < shown_cores; ++ci) {
        const fipsos::LiveCoreTelemetry& c = lt.cores[ci];
        const int ry = cy + 26 + ci * core_step;
        const uint64_t tot = c.active_ticks + c.idle_ticks;
        uint32_t util = (tot > 0ULL) ? static_cast<uint32_t>((c.active_ticks * 100ULL) / tot) : 0U;
        if (util > 100U) util = 100U;

        size_t p = 0;
        p = fmt_str(g_ui.fmt, p, "Core");
        p = fmt_u32(g_ui.fmt, p, static_cast<uint32_t>(ci));
        p = fmt_str(g_ui.fmt, p, (c.online != 0U) ? " [ON] APIC:" : " [--] APIC:");
        p = fmt_u32(g_ui.fmt, p, c.apic_id);
        draw_text(g_ui.fmt, cx + 10, ry + 2, (c.online != 0U) ? 0x00F5D4 : 0x64748B, cx + 168);

        const int bx = cx + 172;
        const int bw = 136;
        const uint32_t bar_col = (util >= 85U) ? 0xF43F5E : (util >= 60U) ? 0xFBBF24 : 0x10B981;
        fill_round_rect(bx, ry + 1, bw, 10, 3, 0x16243B);
        const int fw = (c.online != 0U) ? max_i(4, static_cast<int>((static_cast<uint32_t>(bw) * util) / 100U)) : 0;
        if (fw > 0) fill_round_rect(bx, ry + 1, min_i(bw, fw), 10, 3, bar_col);

        p = 0;
        p = fmt_u32(g_ui.fmt, p, util);
        p = fmt_str(g_ui.fmt, p, "% Act:");
        p = fmt_u64_cap(g_ui.fmt, sizeof(g_ui.fmt), p, c.active_ticks);
        p = fmt_str(g_ui.fmt, p, " Idl:");
        p = fmt_u64_cap(g_ui.fmt, sizeof(g_ui.fmt), p, c.idle_ticks);
        p = fmt_str(g_ui.fmt, p, " Irq:");
        p = fmt_u64_cap(g_ui.fmt, sizeof(g_ui.fmt), p, c.irq_count);
        draw_text(g_ui.fmt, cx + 318, ry + 2, 0xE2E8F0, clip_r);
    }

    // 2. Memoria RAM Fisica HHDM & Kernel Heap
    const int m_y = cy + 98;
    fill_round_rect(cx, m_y, box_w, 92, 6, 0x091220);
    stroke_round_rect(cx, m_y, box_w, 92, 6, 0x1E3A5F);
    draw_text("RAM Fisica HHDM & Heap del Kernel (Cero Cache):", cx + 10, m_y + 7, 0x38BDF8, clip_r);

    const uint64_t total_fr = (lt.total_frames > 0ULL) ? lt.total_frames : 1ULL;
    const uint64_t used_mib = (lt.used_frames * 4ULL) / 1024ULL;
    const uint64_t tot_mib  = (total_fr * 4ULL) / 1024ULL;
    {
        size_t p = 0;
        p = fmt_str(g_ui.fmt, p, "RAM: ");
        p = fmt_u64_cap(g_ui.fmt, sizeof(g_ui.fmt), p, used_mib);
        p = fmt_str(g_ui.fmt, p, "/");
        p = fmt_u64_cap(g_ui.fmt, sizeof(g_ui.fmt), p, tot_mib);
        p = fmt_str(g_ui.fmt, p, "M (");
        p = fmt_u64_cap(g_ui.fmt, sizeof(g_ui.fmt), p, lt.used_frames);
        p = fmt_str(g_ui.fmt, p, "/");
        p = fmt_u64_cap(g_ui.fmt, sizeof(g_ui.fmt), p, lt.total_frames);
        p = fmt_str(g_ui.fmt, p, " fr) Libres:");
        p = fmt_u64_cap(g_ui.fmt, sizeof(g_ui.fmt), p, lt.free_frames);
        draw_text(g_ui.fmt, cx + 10, m_y + 24, 0xE2E8F0, clip_r);
    }
    const int rbar_w = box_w - 20;
    const int rfill_w = max_i(6, static_cast<int>((static_cast<uint64_t>(rbar_w) * lt.used_frames) / total_fr));
    fill_round_rect(cx + 10, m_y + 41, rbar_w, 10, 3, 0x16243B);
    fill_round_rect(cx + 10, m_y + 41, min_i(rbar_w, rfill_w), 10, 3, 0x00F5D4);

    {
        const uint64_t heap_used_kib = lt.heap_used_bytes / 1024ULL;
        const uint64_t heap_tot_kib  = (lt.heap_total_bytes >= 1024ULL) ? (lt.heap_total_bytes / 1024ULL) : 1ULL;
        const uint32_t heap_pct      = static_cast<uint32_t>((heap_used_kib * 100ULL) / heap_tot_kib);
        size_t p = 0;
        p = fmt_str(g_ui.fmt, p, "Heap Ring0: ");
        p = fmt_u64_cap(g_ui.fmt, sizeof(g_ui.fmt), p, heap_used_kib);
        p = fmt_str(g_ui.fmt, p, "/");
        p = fmt_u64_cap(g_ui.fmt, sizeof(g_ui.fmt), p, heap_tot_kib);
        p = fmt_str(g_ui.fmt, p, " KiB (");
        p = fmt_u32(g_ui.fmt, p, heap_pct);
        p = fmt_str(g_ui.fmt, p, "%) | Perfil:");
        p = fmt_str(g_ui.fmt, p, g_ui.bat_profile);
        draw_text(g_ui.fmt, cx + 10, m_y + 57, 0x93C5FD, clip_r);
    }
    {
        size_t p = 0;
        if (lt.last_ram_rw_pages_ok > 0U || lt.last_ram_rw_mbps > 0U) {
            p = fmt_str(g_ui.fmt, p, "Prueba RAM R/W: OK ");
            p = fmt_u32(g_ui.fmt, p, lt.last_ram_rw_pages_ok);
            p = fmt_str(g_ui.fmt, p, " pags 4K @ ");
            p = fmt_u32(g_ui.fmt, p, lt.last_ram_rw_mbps);
            p = fmt_str(g_ui.fmt, p, " MiB/s (dealloc_frame verificado)");
            draw_text(g_ui.fmt, cx + 10, m_y + 74, 0x7AE582, clip_r);
        } else {
            draw_text("Prueba RAM R/W: Pulsa 'Probar RAM R/W' para verificar R/W HHDM real", cx + 10, m_y + 74, 0x94A3B8, clip_r);
        }
    }

    // 3. Procesos Activos, CR3 y Paginas
    const int p_y = cy + 194;
    fill_round_rect(cx, p_y, box_w, 138, 6, 0x091220);
    stroke_round_rect(cx, p_y, box_w, 138, 6, 0x1E3A5F);
    {
        size_t p = 0;
        p = fmt_str(g_ui.fmt, p, "Tabla de Procesos & CR3 en Vivo (Activos=");
        p = fmt_u32(g_ui.fmt, p, lt.active_procs);
        p = fmt_str(g_ui.fmt, p, " | Lanzamientos=");
        p = fmt_u32(g_ui.fmt, p, g_ui.app_launches);
        p = fmt_str(g_ui.fmt, p, ")");
        draw_text(g_ui.fmt, cx + 10, p_y + 7, 0x38BDF8, clip_r);
    }
    draw_text("PID  TID  Nombre         CR3 Phys       Pag4K   Ticks   Est Slot", cx + 10, p_y + 23, 0x64748B, clip_r);

    int drawn_rows = 0;
    for (int i = 0; i < 8 && drawn_rows < 6; ++i) {
        const fipsos::LiveProcTelemetry& pr = lt.procs[i];
        if (pr.state == 0U && pr.pid == 0ULL) continue;
        const int ry = p_y + 39 + drawn_rows * 16;

        size_t p = 0;
        p = fmt_u64_cap(g_ui.fmt, sizeof(g_ui.fmt), p, pr.pid);
        draw_text(g_ui.fmt, cx + 10, ry, 0xF8FAFC, cx + 48);

        p = 0;
        p = fmt_u64_cap(g_ui.fmt, sizeof(g_ui.fmt), p, pr.tid);
        draw_text(g_ui.fmt, cx + 50, ry, 0x94A3B8, cx + 88);

        draw_fixed_proc_name(pr.name, cx + 90, ry, 0xE2E8F0, cx + 206);

        p = 0;
        p = fmt_hex_u64(g_ui.fmt, p, pr.cr3_phys);
        draw_text(g_ui.fmt, cx + 210, ry, 0x00F5D4, cx + 324);

        p = 0;
        p = fmt_u32(g_ui.fmt, p, pr.user_pages);
        draw_text(g_ui.fmt, cx + 330, ry, 0x93C5FD, cx + 388);

        p = 0;
        p = fmt_u32(g_ui.fmt, p, pr.ticks);
        draw_text(g_ui.fmt, cx + 394, ry, 0xE2E8F0, cx + 452);

        draw_text(proc_state_label(pr.state), cx + 458, ry, proc_state_color(pr.state), cx + 492);

        p = 0;
        p = fmt_str(g_ui.fmt, p, "#");
        p = fmt_u32(g_ui.fmt, p, pr.slot_id);
        draw_text(g_ui.fmt, cx + 498, ry, 0x64748B, clip_r);

        ++drawn_rows;
    }

    // 4. Botonera inferior
    const int by = w.y + w.h - 36;
    const int bw = (w.w - 40) / 4;
    const char* prof_lbl = (g_ui.bat_profile[0] == 'p' && g_ui.bat_profile[1] == 'o') ? "Perf: powersave"
                         : (g_ui.bat_profile[0] == 'p' && g_ui.bat_profile[1] == 'e') ? "Perf: perf"
                         : "Perf: balanced";
    draw_button(w.x + 14,              by, bw, 26, "Probar RAM R/W", 0x123254, 0x38BDF8, 0xF8FAFC);
    draw_button(w.x + 14 + (bw + 4),   by, bw, 26, "Lanzar Proceso", 0x064E3B, 0x34D399, 0xF8FAFC);
    draw_button(w.x + 14 + (bw + 4)*2, by, bw, 26, prof_lbl,         0x1E293B, 0xFBBF24, 0xF8FAFC);
    draw_button(w.x + 14 + (bw + 4)*3, by, bw, 26, "Refrescar Vivo", 0x123B34, 0x00F5D4, 0xF8FAFC);
}

bool live_monitor_handle_click(const FloatingWindow& w, int mx, int my) noexcept {
    const int by = w.y + w.h - 36;
    const int bw = (w.w - 40) / 4;
    if (pt_in_rect(mx, my, w.x + 14,              by, bw, 26)) { live_monitor_run_ram_probe(256ULL); return true; }
    if (pt_in_rect(mx, my, w.x + 14 + (bw + 4),   by, bw, 26)) { live_monitor_launch_process();      return true; }
    if (pt_in_rect(mx, my, w.x + 14 + (bw + 4)*2, by, bw, 26)) {
        cycle_power_profile();
        refresh_live_kernel_telemetry();
        return true;
    }
    if (pt_in_rect(mx, my, w.x + 14 + (bw + 4)*3, by, bw, 26)) {
        refresh_live_kernel_telemetry();
        refresh_kernel_telemetry();
        read_bat0_telemetry(false);
        return true;
    }
    return false;
}

} // namespace fipsos::ui
