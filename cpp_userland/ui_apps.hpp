#pragma once
#include "ui_renderer.hpp"

namespace fipsos::ui {

void push_spark_sample(uint8_t v) noexcept;
void refresh_hwmon_file() noexcept;
void refresh_kernel_telemetry() noexcept;

void action_ping_gateway(bool log_status) noexcept;
void action_run_npu() noexcept;
void action_save_disk() noexcept;
void action_spawn_bg_task() noexcept;
void action_kill_task() noexcept;
void action_refresh_net() noexcept;
void action_appcenter_cycle() noexcept;
void action_appcenter_install() noexcept;
void action_appcenter_launch() noexcept;
void action_launch_elf(const char* elf_path, const char* label) noexcept;

void draw_win_appcenter(const FloatingWindow& w, bool focused) noexcept;
bool apps_handle_window_click(int wid, const FloatingWindow& w, int mx, int my) noexcept;

void load_desktop_config() noexcept;
void save_desktop_config() noexcept;
void toggle_desktop_theme() noexcept;

struct BatTelemetry {
    uint32_t capacity_pct;
    bool present;
    bool ac_online;
    bool low_bat;
    char status[16];
    char profile[16];
};

inline void read_sys_str(const char* path, char* out, size_t cap) noexcept {
    if (cap == 0) return;
    out[0] = '\0';
    const int64_t fd = sys_open(path, O_RDONLY);
    if (fd < 0) return;
    const int64_t n = sys_read(static_cast<uint64_t>(fd), out, cap - 1);
    sys_close(static_cast<uint64_t>(fd));
    if (n <= 0) { out[0] = '\0'; return; }
    size_t len = static_cast<size_t>(n);
    while (len > 0 && (out[len - 1] == '\n' || out[len - 1] == '\r' || out[len - 1] == ' ')) --len;
    out[len] = '\0';
}

inline BatTelemetry& bat0_cache_slot() noexcept {
    static BatTelemetry s_cache{0U, false, true, false, "NotPresent", "balanced"};
    return s_cache;
}

inline uint8_t& bat0_cache_sec() noexcept {
    static uint8_t s_sec = 255U;
    return s_sec;
}

inline void sync_bat_ui_cache(const BatTelemetry& b) noexcept {
    g_ui.bat_capacity_pct = b.capacity_pct;
    g_ui.bat_present      = b.present;
    g_ui.bat_ac_online    = b.ac_online;
    g_ui.bat_low          = b.low_bat;
    set_cstr(g_ui.bat_status, sizeof(g_ui.bat_status), b.status);
    set_cstr(g_ui.bat_profile, sizeof(g_ui.bat_profile), b.profile);
}

inline void refresh_ai_guard_cache() noexcept {
    g_ui.ai_guard_raw[0] = '\0';
    g_ui.ai_guard_len = 0;
    const int64_t gfd = sys_open("/mnt/fat0/ai_guard.state", O_RDONLY);
    if (gfd >= 0) {
        const int64_t n = sys_read(static_cast<uint64_t>(gfd), g_ui.ai_guard_raw, sizeof(g_ui.ai_guard_raw) - 1);
        sys_close(static_cast<uint64_t>(gfd));
        if (n > 0) {
            g_ui.ai_guard_len = static_cast<int>(n);
            g_ui.ai_guard_raw[g_ui.ai_guard_len] = '\0';
        }
    }
}

inline BatTelemetry read_bat0_telemetry(bool force = false) noexcept {
    if (!force && bat0_cache_sec() == g_ui.sys_tel.rtc_sec) {
        return bat0_cache_slot();
    }
    BatTelemetry b{0U, false, true, false, "NotPresent", "balanced"};
    char cbuf[12] = {};
    read_sys_str("/sys/class/power_supply/BAT0/capacity", cbuf, sizeof(cbuf));
    if (cbuf[0] >= '0' && cbuf[0] <= '9') {
        uint32_t v = 0;
        for (size_t i = 0; cbuf[i] >= '0' && cbuf[i] <= '9'; ++i) {
            v = v * 10U + static_cast<uint32_t>(cbuf[i] - '0');
        }
        b.capacity_pct = (v > 100U) ? 100U : v;
        b.present = true;
    }
    char sbuf[16] = {};
    read_sys_str("/sys/class/power_supply/BAT0/status", sbuf, sizeof(sbuf));
    if (sbuf[0] != '\0') {
        set_cstr(b.status, sizeof(b.status), sbuf);
        if (sbuf[0] == 'N') b.present = false;
    }
    char pbuf[16] = {};
    read_sys_str("/sys/class/power_supply/BAT0/profile", pbuf, sizeof(pbuf));
    if (pbuf[0] != '\0') set_cstr(b.profile, sizeof(b.profile), pbuf);
    b.ac_online = !b.present || (b.status[0] != 'D');
    b.low_bat = b.present && (b.capacity_pct < 20U);
    bat0_cache_slot() = b;
    bat0_cache_sec() = g_ui.sys_tel.rtc_sec;
    sync_bat_ui_cache(b);
    return b;
}

inline void cycle_power_profile() noexcept {
    const BatTelemetry b = read_bat0_telemetry(true);
    const char* next_prof = "powersave";
    size_t next_len = 9;
    if (b.profile[0] == 'p' && b.profile[1] == 'o') { next_prof = "performance"; next_len = 11; }
    else if (b.profile[0] == 'p' && b.profile[1] == 'e') { next_prof = "balanced"; next_len = 8; }
    const int64_t fd = sys_open("/sys/class/power_supply/BAT0/profile", O_WRONLY);
    if (fd >= 0) {
        sys_write(static_cast<uint64_t>(fd), next_prof, next_len);
        sys_close(static_cast<uint64_t>(fd));
    }
    read_bat0_telemetry(true);
    refresh_hwmon_file();
}

} // namespace fipsos::ui
