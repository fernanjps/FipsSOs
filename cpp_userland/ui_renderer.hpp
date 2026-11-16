#pragma once
#include "fipsos_abi.hpp"

namespace fipsos::ui {

constexpr int kSphereSize = 132;
constexpr float kPi     = 3.14159265f;
constexpr float kTwoPi  = 6.28318531f;
constexpr float kHalfPi = 1.57079633f;

inline float sqrtf_sse2(float x) noexcept {
    if (x <= 0.0f) {
        return 0.0f;
    }
    float out = 0.0f;
    asm volatile("sqrtss %1, %0" : "=x"(out) : "x"(x));
    return out;
}

inline float wrap_pi(float rad) noexcept {
    while (rad > kPi) {
        rad -= kTwoPi;
    }
    while (rad < -kPi) {
        rad += kTwoPi;
    }
    return rad;
}

inline float sinf_fast(float rad) noexcept {
    const float x     = wrap_pi(rad);
    const float b     = 4.0f / kPi;
    const float c     = -4.0f / (kPi * kPi);
    const float abs_x = (x >= 0.0f) ? x : -x;
    const float y     = b * x + c * x * abs_x;
    const float abs_y = (y >= 0.0f) ? y : -y;
    return 0.225f * (y * abs_y - y) + y;
}

inline float cosf_fast(float rad) noexcept {
    return sinf_fast(rad + kHalfPi);
}

inline float clamp01(float v) noexcept {
    if (v < 0.0f) return 0.0f;
    if (v > 1.0f) return 1.0f;
    return v;
}

inline float clamp_f(float v, float lo, float hi) noexcept {
    if (v < lo) return lo;
    if (v > hi) return hi;
    return v;
}

inline int clamp_i(int v, int lo, int hi) noexcept {
    if (v < lo) return lo;
    if (v > hi) return hi;
    return v;
}

inline int min_i(int a, int b) noexcept {
    return (a < b) ? a : b;
}

inline int max_i(int a, int b) noexcept {
    return (a > b) ? a : b;
}

struct Rect {
    int x;
    int y;
    int w;
    int h;
};

inline bool pt_in_rect(int px, int py, int rx, int ry, int rw, int rh) noexcept {
    return px >= rx && px < (rx + rw) && py >= ry && py < (ry + rh);
}

inline bool rects_intersect(const Rect& a, const Rect& b) noexcept {
    return !(a.x + a.w <= b.x || b.x + b.w <= a.x || a.y + a.h <= b.y || b.y + b.h <= a.y);
}

struct FloatingWindow {
    int         id;
    int         x;
    int         y;
    int         w;
    int         h;
    bool        open;
    bool        minimized;
    uint32_t    accent_rgb;
    const char* title;
    int         workspace;
};

struct VfsEntry {
    char     name[48];
    uint8_t  type;
    uint32_t size;
};

struct CompositorState {
    uint32_t* backbuf;
    int       screen_w;
    int       screen_h;
    int       fb_pitch;
    int       bpp;
    int       taskbar_h;
    int       taskbar_y;
    int64_t   fb_fd;
    int64_t   mouse_fd;
    int64_t   net_fd;
    int64_t   accel_fd;
    int64_t   model_fd;
    int64_t   pid;

    Rect clip;

    static constexpr int kMaxW = 2560;
    static constexpr int kMaxH = 1600;
    uint8_t bg_xr[kMaxW];
    uint8_t bg_xg[kMaxW];
    uint8_t bg_xb[kMaxW];
    uint8_t bg_yr[kMaxH];
    uint8_t bg_yg[kMaxH];
    uint8_t bg_yb[kMaxH];

    uint32_t sphere_tile[kSphereSize * kSphereSize];
    float    yaw_rad;
    float    pitch_rad;
    float    light_dir[3];
    uint32_t palette_mode;
    float    simd_trace;
    fipsos::BgHttpdInfo bg_httpd;

    static constexpr int kWinCount = 7;
    FloatingWindow wins[kWinCount];
    int            z_order[kWinCount];
    int            dragging_win;
    int            drag_off_x;
    int            drag_off_y;
    uint32_t       theme_id;
    int            current_ws;

    bool start_open;
    bool should_exit;
    int  hover_dock;
    int  hover_start_tile;

    uint16_t mouse_x;
    uint16_t mouse_y;
    uint8_t  mouse_btns;
    uint8_t  prev_btns;
    uint32_t mouse_seq;

    fipsos::SysTelemetryPacket sys_tel;
    uint16_t last_rtt_us;
    uint32_t npu_energy_milli;
    uint32_t npu_runs;
    uint32_t app_launches;
    uint8_t  sparkline[24];
    uint32_t spark_head;
    char     last_action[56];
    char     fmt[128];

    // Explorer (/mnt/fat0, /disk, /bin, /usr/include)
    static constexpr int kMaxDirEntries   = 64;
    static constexpr int kVisibleDirRows  = 9;
    static constexpr int kMaxPreviewLines = 11;
    static constexpr int kMaxHwmonLines   = 8;

    char     exp_dir[64];
    VfsEntry exp_entries[kMaxDirEntries];
    int      exp_count;
    int      exp_selected;
    int      exp_scroll;
    int      exp_active_tab;
    char     exp_preview_title[48];
    char     exp_preview_lines[kMaxPreviewLines][46];
    int      exp_preview_line_count;
    char     exp_status[64];

    // Live /sys/fipsos/hwmon snapshot lines
    char hwmon_lines[kMaxHwmonLines][68];
    int  hwmon_line_count;

    // App Center & Execution Console state
    int  appcenter_selected;
    bool appcenter_installed[3];
    char appcenter_console[128];
    char appcenter_last_exec[64];

    // Integrated Desktop Terminal state
    static constexpr int kTermMaxLines = 32;
    char term_cwd[64];
    char term_input[96];
    int  term_input_len;
    char     term_lines[kTermMaxLines][76];
    int      term_line_count;
    int      term_scroll;
    char     cfg_status[48];

    // Cached BAT0 & AI Guard status updated outside pure render loops
    uint32_t bat_capacity_pct;
    bool     bat_present;
    bool     bat_ac_online;
    bool     bat_low;
    char     bat_status[16];
    char     bat_profile[16];
    char     ai_guard_raw[48];
    int      ai_guard_len;
    fipsos::FipsosLiveTelemetry live_tel;
    uint64_t live_probe_tsc;
};

extern CompositorState g_ui;

// Freestanding string helpers
size_t str_len(const char* s) noexcept;
bool   str_eq(const char* a, const char* b) noexcept;
void   set_cstr(char* dst, size_t cap, const char* src) noexcept;
size_t fmt_str(char* buf, size_t pos, const char* s) noexcept;
size_t fmt_str_cap(char* buf, size_t cap, size_t pos, const char* s) noexcept;
size_t fmt_u32(char* buf, size_t pos, uint32_t val, size_t cap = 128) noexcept;
size_t fmt_u32_pad2(char* buf, size_t pos, uint32_t val, size_t cap = 128) noexcept;
size_t fmt_hex_byte(char* buf, size_t pos, uint8_t b, size_t cap = 128) noexcept;
size_t fmt_hex_u64(char* buf, size_t pos, uint64_t val, size_t cap = 128) noexcept;

// 2D clipped rendering primitives over /dev/fb0 double buffer
uint32_t blend_rgb(uint32_t dst, uint32_t src, uint32_t alpha) noexcept;
void put_px(int x, int y, uint32_t rgb) noexcept;
void fill_rect(int x, int y, int w, int h, uint32_t rgb) noexcept;
void blend_rect(int x, int y, int w, int h, uint32_t rgb, uint32_t alpha_256) noexcept;
void fill_round_rect(int x, int y, int w, int h, int r, uint32_t rgb) noexcept;
void blend_round_rect(int x, int y, int w, int h, int r, uint32_t rgb, uint32_t alpha_256) noexcept;
void stroke_round_rect(int x, int y, int w, int h, int r, uint32_t rgb) noexcept;
void draw_shadow(int x, int y, int w, int h) noexcept;
void draw_char(char ch, int x, int y, uint32_t fg, int max_x = 0x7FFFFFFF) noexcept;
void draw_text(const char* s, int x, int y, uint32_t fg, int max_x = 0x7FFFFFFF) noexcept;
void draw_text_centered(const char* s, int rx, int ry, int rw, int rh, uint32_t fg) noexcept;
void draw_button(int x, int y, int w, int h, const char* label, uint32_t bg, uint32_t border, uint32_t fg) noexcept;

// Icons, Wallpaper, 3D Sphere & DMA Buffer Flips
void draw_win11_logo(int cx, int cy, int half_sz, uint32_t c_tl, uint32_t c_tr, uint32_t c_bl, uint32_t c_br) noexcept;
void draw_icon_pulse(int x, int y) noexcept;
void draw_icon_orb3d(int x, int y) noexcept;
void draw_icon_folder(int x, int y) noexcept;
void draw_icon_rust_elf(int x, int y) noexcept;
void draw_icon_tty(int x, int y) noexcept;
void draw_mouse_cursor_sprite(int mx, int my, uint8_t btns) noexcept;
void init_wallpaper_luts() noexcept;
void render_wallpaper_clipped() noexcept;
void recompute_sphere_tile() noexcept;
void flip_full() noexcept;
void flip_dirty_rect(const Rect& r) noexcept;

} // namespace fipsos::ui
