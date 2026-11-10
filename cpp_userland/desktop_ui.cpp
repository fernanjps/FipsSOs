#include "fipsos_abi.hpp"

namespace {

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
    if (v < 0.0f) {
        return 0.0f;
    }
    if (v > 1.0f) {
        return 1.0f;
    }
    return v;
}

inline float clamp_f(float v, float lo, float hi) noexcept {
    if (v < lo) {
        return lo;
    }
    if (v > hi) {
        return hi;
    }
    return v;
}

inline int clamp_i(int v, int lo, int hi) noexcept {
    if (v < lo) {
        return lo;
    }
    if (v > hi) {
        return hi;
    }
    return v;
}

inline int min_i(int a, int b) noexcept {
    return (a < b) ? a : b;
}

inline int max_i(int a, int b) noexcept {
    return (a > b) ? a : b;
}

/// Embedded printable ASCII 8x8 bitmap font (`0x20..=0x7E`, 95 glyphs).
constexpr uint8_t kFont8x8[95][8] = {
    {0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00}, // ' '
    {0x18, 0x3C, 0x3C, 0x18, 0x18, 0x00, 0x18, 0x00}, // '!'
    {0x6C, 0x6C, 0x48, 0x00, 0x00, 0x00, 0x00, 0x00}, // '"'
    {0x6C, 0x6C, 0xFE, 0x6C, 0xFE, 0x6C, 0x6C, 0x00}, // '#'
    {0x18, 0x3E, 0x60, 0x3C, 0x06, 0x7C, 0x18, 0x00}, // '$'
    {0x00, 0xC6, 0xCC, 0x18, 0x30, 0x66, 0xC6, 0x00}, // '%'
    {0x38, 0x6C, 0x38, 0x76, 0xDC, 0xCC, 0x76, 0x00}, // '&'
    {0x18, 0x18, 0x30, 0x00, 0x00, 0x00, 0x00, 0x00}, // '\''
    {0x0C, 0x18, 0x30, 0x30, 0x30, 0x18, 0x0C, 0x00}, // '('
    {0x30, 0x18, 0x0C, 0x0C, 0x0C, 0x18, 0x30, 0x00}, // ')'
    {0x00, 0x66, 0x3C, 0xFF, 0x3C, 0x66, 0x00, 0x00}, // '*'
    {0x00, 0x18, 0x18, 0x7E, 0x18, 0x18, 0x00, 0x00}, // '+'
    {0x00, 0x00, 0x00, 0x00, 0x00, 0x18, 0x18, 0x30}, // ','
    {0x00, 0x00, 0x00, 0x7E, 0x00, 0x00, 0x00, 0x00}, // '-'
    {0x00, 0x00, 0x00, 0x00, 0x00, 0x18, 0x18, 0x00}, // '.'
    {0x06, 0x0C, 0x18, 0x30, 0x60, 0xC0, 0x80, 0x00}, // '/'
    {0x7C, 0xC6, 0xCE, 0xDE, 0xF6, 0xE6, 0x7C, 0x00}, // '0'
    {0x18, 0x38, 0x18, 0x18, 0x18, 0x18, 0x7E, 0x00}, // '1'
    {0x7C, 0xC6, 0x06, 0x1C, 0x30, 0x66, 0xFE, 0x00}, // '2'
    {0x7C, 0xC6, 0x06, 0x3C, 0x06, 0xC6, 0x7C, 0x00}, // '3'
    {0x1C, 0x3C, 0x6C, 0xCC, 0xFE, 0x0C, 0x1E, 0x00}, // '4'
    {0xFE, 0xC0, 0xFC, 0x06, 0x06, 0xC6, 0x7C, 0x00}, // '5'
    {0x38, 0x60, 0xC0, 0xFC, 0xC6, 0xC6, 0x7C, 0x00}, // '6'
    {0xFE, 0xC6, 0x0C, 0x18, 0x30, 0x30, 0x30, 0x00}, // '7'
    {0x7C, 0xC6, 0xC6, 0x7C, 0xC6, 0xC6, 0x7C, 0x00}, // '8'
    {0x7C, 0xC6, 0xC6, 0x7E, 0x06, 0x0C, 0x78, 0x00}, // '9'
    {0x00, 0x18, 0x18, 0x00, 0x00, 0x18, 0x18, 0x00}, // ':'
    {0x00, 0x18, 0x18, 0x00, 0x00, 0x18, 0x18, 0x30}, // ';'
    {0x0C, 0x18, 0x30, 0x60, 0x30, 0x18, 0x0C, 0x00}, // '<'
    {0x00, 0x00, 0x7E, 0x00, 0x00, 0x7E, 0x00, 0x00}, // '='
    {0x60, 0x30, 0x18, 0x0C, 0x18, 0x30, 0x60, 0x00}, // '>'
    {0x7C, 0xC6, 0x0C, 0x18, 0x18, 0x00, 0x18, 0x00}, // '?'
    {0x7C, 0xC6, 0xDE, 0xDE, 0xDE, 0xC0, 0x78, 0x00}, // '@'
    {0x38, 0x6C, 0xC6, 0xFE, 0xC6, 0xC6, 0xC6, 0x00}, // 'A'
    {0xFC, 0x66, 0x66, 0x7C, 0x66, 0x66, 0xFC, 0x00}, // 'B'
    {0x3C, 0x66, 0xC0, 0xC0, 0xC0, 0x66, 0x3C, 0x00}, // 'C'
    {0xF8, 0x6C, 0x66, 0x66, 0x66, 0x6C, 0xF8, 0x00}, // 'D'
    {0xFE, 0x62, 0x68, 0x78, 0x68, 0x62, 0xFE, 0x00}, // 'E'
    {0xFE, 0x62, 0x68, 0x78, 0x68, 0x60, 0xF0, 0x00}, // 'F'
    {0x3C, 0x66, 0xC0, 0xC0, 0xCE, 0x66, 0x3E, 0x00}, // 'G'
    {0xC6, 0xC6, 0xC6, 0xFE, 0xC6, 0xC6, 0xC6, 0x00}, // 'H'
    {0x3C, 0x18, 0x18, 0x18, 0x18, 0x18, 0x3C, 0x00}, // 'I'
    {0x1E, 0x0C, 0x0C, 0x0C, 0xCC, 0xCC, 0x78, 0x00}, // 'J'
    {0xE6, 0x66, 0x6C, 0x78, 0x6C, 0x66, 0xE6, 0x00}, // 'K'
    {0xF0, 0x60, 0x60, 0x60, 0x62, 0x66, 0xFE, 0x00}, // 'L'
    {0xC6, 0xEE, 0xFE, 0xFE, 0xD6, 0xC6, 0xC6, 0x00}, // 'M'
    {0xC6, 0xE6, 0xF6, 0xDE, 0xCE, 0xC6, 0xC6, 0x00}, // 'N'
    {0x7C, 0xC6, 0xC6, 0xC6, 0xC6, 0xC6, 0x7C, 0x00}, // 'O'
    {0xFC, 0x66, 0x66, 0x7C, 0x60, 0x60, 0xF0, 0x00}, // 'P'
    {0x7C, 0xC6, 0xC6, 0xC6, 0xD6, 0xDE, 0x7C, 0x0E}, // 'Q'
    {0xFC, 0x66, 0x66, 0x7C, 0x6C, 0x66, 0xE6, 0x00}, // 'R'
    {0x7C, 0xC6, 0x60, 0x38, 0x0C, 0xC6, 0x7C, 0x00}, // 'S'
    {0x7E, 0x7E, 0x5A, 0x18, 0x18, 0x18, 0x3C, 0x00}, // 'T'
    {0xC6, 0xC6, 0xC6, 0xC6, 0xC6, 0xC6, 0x7C, 0x00}, // 'U'
    {0xC6, 0xC6, 0xC6, 0xC6, 0xC6, 0x6C, 0x38, 0x00}, // 'V'
    {0xC6, 0xC6, 0xC6, 0xD6, 0xFE, 0xEE, 0xC6, 0x00}, // 'W'
    {0xC6, 0xC6, 0x6C, 0x38, 0x6C, 0xC6, 0xC6, 0x00}, // 'X'
    {0x66, 0x66, 0x66, 0x3C, 0x18, 0x18, 0x3C, 0x00}, // 'Y'
    {0xFE, 0xC6, 0x8C, 0x18, 0x32, 0x66, 0xFE, 0x00}, // 'Z'
    {0x3C, 0x30, 0x30, 0x30, 0x30, 0x30, 0x3C, 0x00}, // '['
    {0xC0, 0x60, 0x30, 0x18, 0x0C, 0x06, 0x02, 0x00}, // '\\'
    {0x3C, 0x0C, 0x0C, 0x0C, 0x0C, 0x0C, 0x3C, 0x00}, // ']'
    {0x10, 0x38, 0x6C, 0xC6, 0x00, 0x00, 0x00, 0x00}, // '^'
    {0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xFF}, // '_'
    {0x30, 0x18, 0x0C, 0x00, 0x00, 0x00, 0x00, 0x00}, // '`'
    {0x00, 0x00, 0x78, 0x0C, 0x7C, 0xCC, 0x76, 0x00}, // 'a'
    {0xE0, 0x60, 0x7C, 0x66, 0x66, 0x66, 0xDC, 0x00}, // 'b'
    {0x00, 0x00, 0x7C, 0xC6, 0xC0, 0xC6, 0x7C, 0x00}, // 'c'
    {0x1C, 0x0C, 0x7C, 0xCC, 0xCC, 0xCC, 0x76, 0x00}, // 'd'
    {0x00, 0x00, 0x7C, 0xC6, 0xFE, 0xC0, 0x7C, 0x00}, // 'e'
    {0x3C, 0x66, 0x60, 0xF8, 0x60, 0x60, 0xF0, 0x00}, // 'f'
    {0x00, 0x00, 0x76, 0xCC, 0xCC, 0x7C, 0x0C, 0xF8}, // 'g'
    {0xE0, 0x60, 0x6C, 0x76, 0x66, 0x66, 0xE6, 0x00}, // 'h'
    {0x18, 0x00, 0x38, 0x18, 0x18, 0x18, 0x3C, 0x00}, // 'i'
    {0x06, 0x00, 0x06, 0x06, 0x06, 0x66, 0x66, 0x3C}, // 'j'
    {0xE0, 0x60, 0x66, 0x6C, 0x78, 0x6C, 0xE6, 0x00}, // 'k'
    {0x38, 0x18, 0x18, 0x18, 0x18, 0x18, 0x3C, 0x00}, // 'l'
    {0x00, 0x00, 0xEC, 0xFE, 0xD6, 0xC6, 0xC6, 0x00}, // 'm'
    {0x00, 0x00, 0xDC, 0x66, 0x66, 0x66, 0x66, 0x00}, // 'n'
    {0x00, 0x00, 0x7C, 0xC6, 0xC6, 0xC6, 0x7C, 0x00}, // 'o'
    {0x00, 0x00, 0xDC, 0x66, 0x66, 0x7C, 0x60, 0xF0}, // 'p'
    {0x00, 0x00, 0x76, 0xCC, 0xCC, 0x7C, 0x0C, 0x1E}, // 'q'
    {0x00, 0x00, 0xDC, 0x76, 0x66, 0x60, 0xF0, 0x00}, // 'r'
    {0x00, 0x00, 0x7E, 0xC0, 0x7C, 0x06, 0xFC, 0x00}, // 's'
    {0x10, 0x30, 0x7C, 0x30, 0x30, 0x34, 0x18, 0x00}, // 't'
    {0x00, 0x00, 0xCC, 0xCC, 0xCC, 0xCC, 0x76, 0x00}, // 'u'
    {0x00, 0x00, 0xC6, 0xC6, 0xC6, 0x6C, 0x38, 0x00}, // 'v'
    {0x00, 0x00, 0xC6, 0xD6, 0xFE, 0xFE, 0x6C, 0x00}, // 'w'
    {0x00, 0x00, 0xC6, 0x6C, 0x38, 0x6C, 0xC6, 0x00}, // 'x'
    {0x00, 0x00, 0xC6, 0xC6, 0xCE, 0x76, 0x06, 0x7C}, // 'y'
    {0x00, 0x00, 0xFE, 0x98, 0x30, 0x62, 0xFE, 0x00}, // 'z'
    {0x0E, 0x18, 0x18, 0x70, 0x18, 0x18, 0x0E, 0x00}, // '{'
    {0x18, 0x18, 0x18, 0x00, 0x18, 0x18, 0x18, 0x00}, // '|'
    {0x70, 0x18, 0x18, 0x0E, 0x18, 0x18, 0x70, 0x00}, // '}'
    {0x76, 0xDC, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00}  // '~'
};

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

inline Rect union_rect(const Rect& a, const Rect& b) noexcept {
    if (a.w <= 0 || a.h <= 0) {
        return b;
    }
    if (b.w <= 0 || b.h <= 0) {
        return a;
    }
    const int x0 = min_i(a.x, b.x);
    const int y0 = min_i(a.y, b.y);
    const int x1 = max_i(a.x + a.w, b.x + b.w);
    const int y1 = max_i(a.y + a.h, b.y + b.h);
    return Rect{x0, y0, x1 - x0, y1 - y0};
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
};

/// Compact Ring 3 static state (`< 78 KiB` total `.bss`, zero heap allocations).
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

    // 1D Aurora Wallpaper LUTs (supports dynamically up to 2560x1600 display modes)
    static constexpr int kMaxW = 2560;
    static constexpr int kMaxH = 1600;
    uint8_t bg_xr[kMaxW];
    uint8_t bg_xg[kMaxW];
    uint8_t bg_xb[kMaxW];
    uint8_t bg_yr[kMaxH];
    uint8_t bg_yg[kMaxH];
    uint8_t bg_yb[kMaxH];

    // Cached 132x132 3D Phong sphere tile (recomputed only when 3D camera/light changes)
    uint32_t sphere_tile[kSphereSize * kSphereSize];
    float    yaw_rad;
    float    pitch_rad;
    float    light_dir[3];
    uint32_t palette_mode;

    // Window manager state (4 floating windows, Z-order back->front)
    FloatingWindow wins[4];
    int            z_order[4];
    int            dragging_win;
    int            drag_off_x;
    int            drag_off_y;

    bool start_open;
    bool should_exit;
    int  hover_dock;
    int  hover_start_tile;

    // Pointer state
    uint16_t mouse_x;
    uint16_t mouse_y;
    uint8_t  mouse_btns;
    uint8_t  prev_btns;
    uint32_t mouse_seq;

    // Live hardware & kernel telemetry
    fipsos::SysTelemetryPacket sys_tel;
    uint16_t last_rtt_us;
    uint32_t npu_energy_milli;
    uint32_t npu_runs;
    uint32_t app_launches;
    uint8_t  sparkline[24];
    uint32_t spark_head;
    char     last_action[56];
    char     fmt[128];

    // FASE v1.8 & v2.2: Web Browser, TCP Sockets & Storage state
    int      browser_bookmark; // 1 = host_test.txt, 2 = index.html, 3 = ai_model_v2
    char     browser_url[48];
    char     browser_status[48];
    char     browser_downloaded_body[256];
    int      selected_disk_file;

    // FASE v2.4: App Center & Network Software Manager
    int      appcenter_selected; // 0 = sample_tool.elf, 1 = benchmark_simd.elf, 2 = editor_kilo.elf
    bool     appcenter_installed[3];
    char     appcenter_console[128];
    char     appcenter_last_exec[64];
};

CompositorState g_ui;

// --- String formatting helpers (freestanding, zero libc) ---

size_t str_len(const char* s) noexcept {
    size_t n = 0;
    while (s[n] != '\0') {
        n += 1;
    }
    return n;
}

void set_cstr(char* dst, size_t cap, const char* src) noexcept {
    if (cap == 0) {
        return;
    }
    size_t i = 0;
    while (i + 1 < cap && src[i] != '\0') {
        dst[i] = src[i];
        i += 1;
    }
    dst[i] = '\0';
}

size_t fmt_str(char* buf, size_t pos, const char* s) noexcept {
    while (*s != '\0' && pos < 126) {
        buf[pos++] = *s++;
    }
    buf[pos] = '\0';
    return pos;
}

size_t fmt_u32(char* buf, size_t pos, uint32_t val) noexcept {
    char tmp[12];
    size_t n = 0;
    if (val == 0) {
        tmp[n++] = '0';
    } else {
        while (val > 0 && n < sizeof(tmp)) {
            tmp[n++] = static_cast<char>('0' + (val % 10U));
            val /= 10U;
        }
    }
    while (n > 0 && pos < 126) {
        buf[pos++] = tmp[--n];
    }
    buf[pos] = '\0';
    return pos;
}

size_t fmt_u32_pad2(char* buf, size_t pos, uint32_t val) noexcept {
    if (val < 10U && pos < 126) {
        buf[pos++] = '0';
    }
    return fmt_u32(buf, pos, val);
}

[[maybe_unused]] static size_t fmt_hex_byte(char* buf, size_t pos, uint8_t b) noexcept {
    constexpr char kHex[] = "0123456789ABCDEF";
    if (pos + 2 < 126) {
        buf[pos++] = kHex[(b >> 4) & 0xF];
        buf[pos++] = kHex[b & 0xF];
        buf[pos]   = '\0';
    }
    return pos;
}

size_t fmt_hex_u64(char* buf, size_t pos, uint64_t val) noexcept {
    constexpr char kHex[] = "0123456789abcdef";
    pos = fmt_str(buf, pos, "0x");
    if (val == 0ULL) {
        if (pos < 126) {
            buf[pos++] = '0';
            buf[pos]   = '\0';
        }
        return pos;
    }
    char tmp[16];
    size_t n = 0;
    while (val > 0ULL && n < sizeof(tmp)) {
        tmp[n++] = kHex[val & 0xFULL];
        val >>= 4;
    }
    while (n > 0 && pos < 126) {
        buf[pos++] = tmp[--n];
    }
    buf[pos] = '\0';
    return pos;
}

// --- Low-level Zero-Copy Backbuffer Primitives with Scissor Clipping ---

inline uint32_t blend_rgb(uint32_t dst, uint32_t src, uint32_t alpha) noexcept {
    const uint32_t inv = 256U - alpha;
    const uint32_t r   = (((src >> 16) & 0xFFU) * alpha + ((dst >> 16) & 0xFFU) * inv) >> 8;
    const uint32_t g   = (((src >> 8) & 0xFFU) * alpha + ((dst >> 8) & 0xFFU) * inv) >> 8;
    const uint32_t b   = ((src & 0xFFU) * alpha + (dst & 0xFFU) * inv) >> 8;
    return (r << 16) | (g << 8) | b;
}

inline void put_px(int x, int y, uint32_t rgb) noexcept {
    if (x >= 0 && x < g_ui.screen_w &&
        y >= 0 && y < g_ui.screen_h &&
        x >= g_ui.clip.x && x < (g_ui.clip.x + g_ui.clip.w) &&
        y >= g_ui.clip.y && y < (g_ui.clip.y + g_ui.clip.h)) {
        const int pitch_pixels = g_ui.fb_pitch / 4;
        g_ui.backbuf[y * pitch_pixels + x] = rgb;
    }
}

void fill_rect(int x, int y, int w, int h, uint32_t rgb) noexcept {
    const int x0 = max_i(max_i(x, g_ui.clip.x), 0);
    const int y0 = max_i(max_i(y, g_ui.clip.y), 0);
    const int x1 = min_i(min_i(x + w, g_ui.clip.x + g_ui.clip.w), g_ui.screen_w);
    const int y1 = min_i(min_i(y + h, g_ui.clip.y + g_ui.clip.h), g_ui.screen_h);
    if (x0 >= x1 || y0 >= y1) {
        return;
    }
    const int pitch_pixels = g_ui.fb_pitch / 4;
    for (int py = y0; py < y1; ++py) {
        uint32_t* row = g_ui.backbuf + py * pitch_pixels;
        for (int px = x0; px < x1; ++px) {
            row[px] = rgb;
        }
    }
}

void blend_rect(int x, int y, int w, int h, uint32_t rgb, uint32_t alpha_256) noexcept {
    const int x0 = max_i(max_i(x, g_ui.clip.x), 0);
    const int y0 = max_i(max_i(y, g_ui.clip.y), 0);
    const int x1 = min_i(min_i(x + w, g_ui.clip.x + g_ui.clip.w), g_ui.screen_w);
    const int y1 = min_i(min_i(y + h, g_ui.clip.y + g_ui.clip.h), g_ui.screen_h);
    if (x0 >= x1 || y0 >= y1) {
        return;
    }
    const int pitch_pixels = g_ui.fb_pitch / 4;
    for (int py = y0; py < y1; ++py) {
        uint32_t* row = g_ui.backbuf + py * pitch_pixels;
        for (int px = x0; px < x1; ++px) {
            row[px] = blend_rgb(row[px], rgb, alpha_256);
        }
    }
}

inline int corner_inset(int dy_from_edge, int r) noexcept {
    // Smooth discrete circle offsets for r <= 8
    if (r <= 0 || dy_from_edge >= r) {
        return 0;
    }
    const int d = r - 1 - dy_from_edge;
    // Integer circle chord approximation
    int inset = 0;
    while ((r - inset - 1) * (r - inset - 1) + d * d > r * r && inset < r) {
        inset += 1;
    }
    return inset;
}

void fill_round_rect(int x, int y, int w, int h, int r, uint32_t rgb) noexcept {
    if (w <= 0 || h <= 0) {
        return;
    }
    const int y0 = max_i(y, g_ui.clip.y);
    const int y1 = min_i(y + h, g_ui.clip.y + g_ui.clip.h);
    const int pitch_pixels = g_ui.fb_pitch / 4;
    for (int py = y0; py < y1; ++py) {
        const int top_d = py - y;
        const int bot_d = (y + h - 1) - py;
        const int edge_d = min_i(top_d, bot_d);
        const int inset  = corner_inset(edge_d, r);
        const int x0     = max_i(x + inset, g_ui.clip.x);
        const int x1     = min_i(x + w - inset, g_ui.clip.x + g_ui.clip.w);
        if (x0 < x1) {
            uint32_t* row = g_ui.backbuf + py * pitch_pixels;
            for (int px = x0; px < x1; ++px) {
                row[px] = rgb;
            }
        }
    }
}

void blend_round_rect(int x, int y, int w, int h, int r, uint32_t rgb, uint32_t alpha_256) noexcept {
    if (w <= 0 || h <= 0) {
        return;
    }
    const int y0 = max_i(y, g_ui.clip.y);
    const int y1 = min_i(y + h, g_ui.clip.y + g_ui.clip.h);
    const int pitch_pixels = g_ui.fb_pitch / 4;
    for (int py = y0; py < y1; ++py) {
        const int top_d  = py - y;
        const int bot_d  = (y + h - 1) - py;
        const int edge_d = min_i(top_d, bot_d);
        const int inset  = corner_inset(edge_d, r);
        const int x0     = max_i(x + inset, g_ui.clip.x);
        const int x1     = min_i(x + w - inset, g_ui.clip.x + g_ui.clip.w);
        if (x0 < x1) {
            uint32_t* row = g_ui.backbuf + py * pitch_pixels;
            for (int px = x0; px < x1; ++px) {
                row[px] = blend_rgb(row[px], rgb, alpha_256);
            }
        }
    }
}

void stroke_round_rect(int x, int y, int w, int h, int r, uint32_t rgb) noexcept {
    if (w <= 2 || h <= 2) {
        return;
    }
    for (int py = y; py < y + h; ++py) {
        const int top_d  = py - y;
        const int bot_d  = (y + h - 1) - py;
        const int edge_d = min_i(top_d, bot_d);
        const int inset  = corner_inset(edge_d, r);
        if (py == y || py == y + h - 1) {
            fill_rect(x + inset, py, w - inset * 2, 1, rgb);
        } else {
            const int prev_d     = min_i(py - 1 - y, (y + h - 1) - (py - 1));
            const int next_d     = min_i(py + 1 - y, (y + h - 1) - (py + 1));
            const int prev_inset = corner_inset(prev_d, r);
            const int next_inset = corner_inset(next_d, r);
            const int span       = max_i(1, max_i(prev_inset - inset, next_inset - inset));
            fill_rect(x + inset, py, span, 1, rgb);
            fill_rect(x + w - inset - span, py, span, 1, rgb);
        }
    }
}

void draw_shadow(int x, int y, int w, int h) noexcept {
    blend_round_rect(x + 4, y + 5, w, h, 8, 0x000308, 120);
    blend_round_rect(x + 2, y + 3, w, h, 8, 0x00050C, 80);
}

void draw_char(char ch, int x, int y, uint32_t fg, int max_x = 0x7FFFFFFF) noexcept {
    const int limit_x = min_i(max_x, g_ui.screen_w - 8);
    if (x < 0 || x > limit_x ||
        x + 8 <= g_ui.clip.x || x >= g_ui.clip.x + g_ui.clip.w ||
        y < 0 || y + 8 > g_ui.screen_h ||
        y + 8 <= g_ui.clip.y || y >= g_ui.clip.y + g_ui.clip.h) {
        return;
    }
    const uint8_t uc  = static_cast<uint8_t>(ch);
    const size_t  idx = (uc >= 0x20U && uc <= 0x7EU) ? static_cast<size_t>(uc - 0x20U) : 0U;
    const uint8_t* g  = kFont8x8[idx];
    for (int gy = 0; gy < 8; ++gy) {
        const uint8_t bits = g[gy];
        if (bits == 0) {
            continue;
        }
        for (int gx = 0; gx < 8; ++gx) {
            if ((bits & (0x80U >> gx)) != 0U) {
                put_px(x + gx, y + gy, fg);
            }
        }
    }
}

void draw_text(const char* s, int x, int y, uint32_t fg, int max_x = 0x7FFFFFFF) noexcept {
    const int limit_x = min_i(max_x, g_ui.screen_w - 8);
    int cx = x;
    while (*s != '\0') {
        if (cx > limit_x) {
            break;
        }
        draw_char(*s, cx, y, fg, limit_x);
        cx += 8;
        ++s;
    }
}

void draw_text_centered(const char* s, int rx, int ry, int rw, int rh, uint32_t fg) noexcept {
    const int tw = static_cast<int>(str_len(s)) * 8;
    const int tx = rx + (rw - tw) / 2;
    const int ty = ry + (rh - 8) / 2;
    draw_text(s, tx, ty, fg);
}

void draw_button(int x, int y, int w, int h, const char* label, uint32_t bg, uint32_t border, uint32_t fg) noexcept {
    const bool hovered = pt_in_rect(g_ui.mouse_x, g_ui.mouse_y, x, y, w, h);
    const uint32_t fill = hovered ? blend_rgb(bg, 0xFFFFFF, 38) : bg;
    const uint32_t brd  = hovered ? 0x00F5D4 : border;
    fill_round_rect(x, y, w, h, 5, fill);
    stroke_round_rect(x, y, w, h, 5, brd);
    draw_text_centered(label, x, y, w, h, fg);
}

// --- Custom Crisp Vector/Pixel Icons (Windows 11 Style) ---

void draw_win11_logo(int cx, int cy, int half_sz, uint32_t c_tl, uint32_t c_tr, uint32_t c_bl, uint32_t c_br) noexcept {
    const int q = half_sz - 1;
    fill_rect(cx - half_sz, cy - half_sz, q, q, c_tl);
    fill_rect(cx + 1,       cy - half_sz, q, q, c_tr);
    fill_rect(cx - half_sz, cy + 1,       q, q, c_bl);
    fill_rect(cx + 1,       cy + 1,       q, q, c_br);
}

void draw_icon_pulse(int x, int y) noexcept {
    fill_round_rect(x, y, 20, 20, 4, 0x0A2E3D);
    stroke_round_rect(x, y, 20, 20, 4, 0x00F5D4);
    fill_rect(x + 3,  y + 13, 3, 4,  0x00E5FF);
    fill_rect(x + 7,  y + 8,  3, 9,  0x00F5D4);
    fill_rect(x + 11, y + 5,  3, 12, 0x38BDF8);
    fill_rect(x + 15, y + 10, 3, 7,  0x7AE582);
}

void draw_icon_orb3d(int x, int y) noexcept {
    fill_round_rect(x, y, 20, 20, 10, 0x1A1B4B);
    fill_round_rect(x + 2, y + 2, 16, 16, 8, 0x3B82F6);
    fill_round_rect(x + 4, y + 4, 10, 10, 5, 0x67E8F9);
    fill_rect(x + 6, y + 6, 3, 3, 0xFFFFFF);
}

void draw_icon_folder(int x, int y) noexcept {
    fill_round_rect(x + 1, y + 3, 8, 4, 2, 0xF59E0B);
    fill_round_rect(x + 1, y + 5, 18, 12, 3, 0xFBBF24);
    fill_rect(x + 2, y + 7, 16, 2, 0xFDE68A);
}

void draw_icon_rust_elf(int x, int y) noexcept {
    fill_round_rect(x, y, 20, 20, 4, 0x3B1A10);
    stroke_round_rect(x, y, 20, 20, 4, 0xFB923C);
    draw_text("RS", x + 2, y + 6, 0xFDBA74);
}

void draw_icon_tty(int x, int y) noexcept {
    fill_round_rect(x, y, 20, 20, 4, 0x111827);
    stroke_round_rect(x, y, 20, 20, 4, 0x94A3B8);
    draw_text(">_", x + 2, y + 6, 0x38BDF8);
}

// --- Wallpaper LUT & 3D SSE2 Sphere Generator ---

void init_wallpaper_luts() noexcept {
    const int w = (g_ui.screen_w > 0 && g_ui.screen_w <= CompositorState::kMaxW) ? g_ui.screen_w : 1024;
    const int h = (g_ui.screen_h > 0 && g_ui.screen_h <= CompositorState::kMaxH) ? g_ui.screen_h : 768;
    for (int x = 0; x < w; ++x) {
        const float fx    = static_cast<float>(x) * (1.0f / static_cast<float>(w));
        const float bloom = sinf_fast((fx - 0.15f) * kPi);
        const float bpos  = (bloom > 0.0f) ? bloom : 0.0f;
        g_ui.bg_xr[x]     = static_cast<uint8_t>(6.0f + bpos * 18.0f);
        g_ui.bg_xg[x]     = static_cast<uint8_t>(14.0f + bpos * 44.0f);
        g_ui.bg_xb[x]     = static_cast<uint8_t>(30.0f + bpos * 72.0f);
    }
    for (int y = 0; y < h; ++y) {
        const float fy   = static_cast<float>(y) * (1.0f / static_cast<float>(h));
        const float wave = sinf_fast((1.0f - fy) * 2.2f);
        const float wpos = (wave > 0.0f) ? wave : 0.0f;
        g_ui.bg_yr[y]    = static_cast<uint8_t>(4.0f + fy * 10.0f + wpos * 8.0f);
        g_ui.bg_yg[y]    = static_cast<uint8_t>(10.0f + (1.0f - fy) * 28.0f + wpos * 22.0f);
        g_ui.bg_yb[y]    = static_cast<uint8_t>(22.0f + (1.0f - fy) * 48.0f + wpos * 34.0f);
    }
}

void render_wallpaper_clipped() noexcept {
    const int x0 = max_i(0, g_ui.clip.x);
    const int y0 = max_i(0, g_ui.clip.y);
    const int x1 = min_i(g_ui.screen_w, g_ui.clip.x + g_ui.clip.w);
    const int y1 = min_i(g_ui.screen_h, g_ui.clip.y + g_ui.clip.h);
    if (x0 >= x1 || y0 >= y1) {
        return;
    }

    const int pitch_pixels = g_ui.fb_pitch / 4;
    for (int y = y0; y < y1; ++y) {
        uint32_t*      row = g_ui.backbuf + y * pitch_pixels;
        const uint32_t yr  = g_ui.bg_yr[y];
        const uint32_t yg  = g_ui.bg_yg[y];
        const uint32_t yb  = g_ui.bg_yb[y];
        for (int x = x0; x < x1; ++x) {
            // Subtle Windows 11 diagonal silk fold highlight
            const int fold = ((x * 3 + y * 5) & 511);
            const uint32_t rib = (fold < 256) ? static_cast<uint32_t>(fold >> 4)
                                              : static_cast<uint32_t>((511 - fold) >> 4);
            const uint32_t r = min_i(255, static_cast<int>(yr + g_ui.bg_xr[x] + (rib >> 1)));
            const uint32_t g = min_i(255, static_cast<int>(yg + g_ui.bg_xg[x] + rib));
            const uint32_t b = min_i(255, static_cast<int>(yb + g_ui.bg_xb[x] + rib + (rib >> 1)));
            row[x] = (r << 16) | (g << 8) | b;
        }
    }
}

void recompute_sphere_tile() noexcept {
    constexpr float cx   = static_cast<float>(kSphereSize) * 0.5f;
    constexpr float cy   = static_cast<float>(kSphereSize) * 0.5f;
    constexpr float rad  = 52.0f;
    constexpr float invr = 1.0f / rad;
    constexpr float halo = (rad + 4.0f) * (rad + 4.0f);

    const float cyaw   = cosf_fast(g_ui.yaw_rad);
    const float syaw   = sinf_fast(g_ui.yaw_rad);
    const float cpitch = cosf_fast(g_ui.pitch_rad);
    const float spitch = sinf_fast(g_ui.pitch_rad);

    const float lx = g_ui.light_dir[0];
    const float ly = g_ui.light_dir[1];
    const float lz = g_ui.light_dir[2];

    float base_r = 0.08f, base_g = 0.72f, base_b = 0.96f;
    if ((g_ui.palette_mode & 3U) == 1U) {
        base_r = 0.10f; base_g = 0.88f; base_b = 0.56f;
    } else if ((g_ui.palette_mode & 3U) == 2U) {
        base_r = 0.76f; base_g = 0.36f; base_b = 0.98f;
    } else if ((g_ui.palette_mode & 3U) == 3U) {
        base_r = 0.96f; base_g = 0.64f; base_b = 0.16f;
    }

    for (int py = 0; py < kSphereSize; ++py) {
        const float dy = static_cast<float>(py) - cy;
        for (int px = 0; px < kSphereSize; ++px) {
            const float dx = static_cast<float>(px) - cx;
            const float d2 = dx * dx + dy * dy;

            if (d2 <= rad * rad) {
                const float sx = dx * invr;
                const float sy = -dy * invr;
                const float sz = sqrtf_sse2(1.0f - (sx * sx + sy * sy));

                // Rotate surface normal for visible meridian bands
                const float rx = sx * cyaw + sz * syaw;
                const float rz = -sx * syaw + sz * cyaw;
                const float ry = sy * cpitch - rz * spitch;

                const float band = 0.84f + 0.16f * sinf_fast(rx * 9.0f + ry * 7.0f);
                const float ndotl = clamp01(sx * lx + (-sy) * (-ly) + sz * lz);

                // Blinn-Phong half-vector specular (V = [0, 0, 1])
                const float hx    = lx;
                const float hy    = -ly;
                const float hz    = lz + 1.0f;
                const float hinv  = 1.0f / sqrtf_sse2(hx * hx + hy * hy + hz * hz);
                const float ndoth = clamp01(sx * (hx * hinv) + (-sy) * (hy * hinv) + sz * (hz * hinv));
                float spec = ndoth * ndoth;
                spec = spec * spec;
                spec = spec * spec; // ^8
                spec = spec * spec; // ^16

                const float rim = (1.0f - sz) * (1.0f - sz) * 0.45f;
                const float lit = 0.16f + 0.78f * ndotl;

                const uint32_t r = static_cast<uint32_t>(clamp01(base_r * band * lit + spec * 0.92f + rim * 0.20f) * 255.0f);
                const uint32_t g = static_cast<uint32_t>(clamp01(base_g * band * lit + spec * 0.95f + rim * 0.65f) * 255.0f);
                const uint32_t b = static_cast<uint32_t>(clamp01(base_b * band * lit + spec * 1.00f + rim * 0.95f) * 255.0f);
                g_ui.sphere_tile[py * kSphereSize + px] = (r << 16) | (g << 8) | b;
            } else if (d2 <= halo) {
                g_ui.sphere_tile[py * kSphereSize + px] = 0x103454;
            } else {
                const bool grid = ((px & 15) == 0) || ((py & 15) == 0);
                g_ui.sphere_tile[py * kSphereSize + px] = grid ? 0x0E1F38 : 0x081224;
            }
        }
    }
}

// --- Hardware / Kernel Telemetry Refresh & Actions ---

void push_spark_sample(uint8_t v) noexcept {
    g_ui.sparkline[g_ui.spark_head % 24U] = v;
    g_ui.spark_head += 1U;
}

void refresh_kernel_telemetry() noexcept {
    if (g_ui.fb_fd >= 0) {
        fipsos::sys_ioctl(
            static_cast<uint64_t>(g_ui.fb_fd),
            fipsos::VFS_IOCTL_SYS_TELEMETRY,
            reinterpret_cast<uint64_t>(&g_ui.sys_tel)
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
            const uint8_t bar = static_cast<uint8_t>(clamp_i(14 + static_cast<int>((nt.rtt_us / 20U) & 27U), 8, 42));
            push_spark_sample(bar);
            if (log_status) {
                set_cstr(g_ui.last_action, sizeof(g_ui.last_action), "Ping ARP+ICMP 10.0.2.2 OK");
            }
        }
    }
    refresh_kernel_telemetry();
}

void action_http_wget() noexcept {
    if (g_ui.net_fd >= 0) {
        const int64_t bytes = fipsos::sys_ioctl(
            static_cast<uint64_t>(g_ui.net_fd),
            fipsos::NET_IOCTL_HTTP_FETCH,
            1ULL
        );
        if (bytes > 0) {
            push_spark_sample(40);
            size_t p = 0;
            p = fmt_str(g_ui.fmt, p, "HTTP wget OK (");
            p = fmt_u32(g_ui.fmt, p, static_cast<uint32_t>(bytes));
            p = fmt_str(g_ui.fmt, p, " B en /downloads)");
            set_cstr(g_ui.last_action, sizeof(g_ui.last_action), g_ui.fmt);
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
            const uint8_t bar = static_cast<uint8_t>(clamp_i(16 + static_cast<int>((score / 220) % 26), 10, 42));
            push_spark_sample(bar);
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
            set_cstr(g_ui.last_action, sizeof(g_ui.last_action), "SATA DiskFS: /disk/persistent_note.txt guardado OK");
        } else {
            set_cstr(g_ui.last_action, sizeof(g_ui.last_action), "SATA DiskFS: Error al sincronizar");
        }
    }
    refresh_kernel_telemetry();
}

void action_spawn_bg_task() noexcept {
    if (g_ui.fb_fd >= 0) {
        fipsos::sys_ioctl(static_cast<uint64_t>(g_ui.fb_fd), fipsos::VFS_IOCTL_TASK_CONTROL, 1ULL);
        push_spark_sample(36);
        set_cstr(g_ui.last_action, sizeof(g_ui.last_action), "Task Manager: bg_downloader PID 1051 iniciado");
    }
    refresh_kernel_telemetry();
}

void action_kill_task() noexcept {
    if (g_ui.fb_fd >= 0) {
        if (g_ui.sys_tel.bg_task_state == 1) {
            fipsos::sys_ioctl(static_cast<uint64_t>(g_ui.fb_fd), fipsos::VFS_IOCTL_TASK_CONTROL, 2ULL);
            set_cstr(g_ui.last_action, sizeof(g_ui.last_action), "Task Manager: bg_downloader PID 1051 terminado");
        } else {
            fipsos::sys_ioctl(static_cast<uint64_t>(g_ui.fb_fd), fipsos::VFS_IOCTL_TASK_CONTROL, 3ULL);
            set_cstr(g_ui.last_action, sizeof(g_ui.last_action), "Task Manager: simd-worker terminado (SIGKILL)");
        }
        push_spark_sample(20);
    }
    refresh_kernel_telemetry();
}

void action_refresh_net() noexcept {
    action_ping_gateway(false);
    refresh_kernel_telemetry();
    push_spark_sample(28);
    set_cstr(g_ui.last_action, sizeof(g_ui.last_action), "Red PCIe eth0: Telemetria refrescada (Link UP 1000 Mbps)");
}

void action_browser_get() noexcept {
    const char* path = "/host_test.txt";
    const char* dest_file = "/disk/host_test.txt";

    if (g_ui.browser_bookmark == 1) {
        path = "/host_test.txt";
        dest_file = "/disk/host_test.txt";
    } else if (g_ui.browser_bookmark == 2) {
        path = "/index.html";
        dest_file = "/disk/index.html";
    } else {
        path = "/ai_model_v2.bin";
        dest_file = "/disk/ai_model_v2.bin";
    }

    // 1. Crear socket POSIX real (SYS_SOCKET 41)
    const int64_t sock_fd = fipsos::sys_socket(fipsos::AF_INET, fipsos::SOCK_STREAM, 0);
    if (sock_fd < 0) {
        set_cstr(g_ui.browser_status, sizeof(g_ui.browser_status), "Error: socket() fallo");
        set_cstr(g_ui.last_action, sizeof(g_ui.last_action), "Red: socket() no disponible");
        refresh_kernel_telemetry();
        return;
    }

    // 2. Conectar a 10.0.2.2:8080 (Three-Way Handshake TCP via e1000e DMA)
    fipsos::SockAddrIn server_addr{};
    server_addr.sin_family = fipsos::AF_INET;
    // Puerto 8080 en formato big-endian
    server_addr.sin_port = static_cast<uint16_t>(((8080U & 0xFFU) << 8) | ((8080U >> 8) & 0xFFU));
    // IP 10.0.2.2 en orden de red
    const uint8_t ip[4] = {10, 0, 2, 2};
    for (size_t i = 0; i < 4; ++i) {
        reinterpret_cast<uint8_t*>(&server_addr.sin_addr.s_addr)[i] = ip[i];
    }

    const int64_t conn_res = fipsos::sys_connect(sock_fd, &server_addr, sizeof(server_addr));
    if (conn_res < 0) {
        fipsos::sys_close(static_cast<uint64_t>(sock_fd));
        set_cstr(g_ui.browser_status, sizeof(g_ui.browser_status), "Error: connect(10.0.2.2:8080)");
        set_cstr(g_ui.last_action, sizeof(g_ui.last_action), "Red: Host 10.0.2.2:8080 inalcanzable");
        refresh_kernel_telemetry();
        return;
    }

    // 3. Enviar solicitud HTTP GET via sys_sendto (SYS_SENDTO 44)
    char req_buf[256];
    size_t req_len = 0;
    req_len = fmt_str(req_buf, req_len, "GET ");
    req_len = fmt_str(req_buf, req_len, path);
    req_len = fmt_str(req_buf, req_len, " HTTP/1.0\r\nHost: 10.0.2.2:8080\r\nUser-Agent: FipSOs-Desktop/2.2\r\n\r\n");

    fipsos::sys_sendto(sock_fd, req_buf, req_len, 0);

    // 4. Recibir respuesta TCP via e1000e DMA: sys_recvfrom (SYS_RECVFROM 45)
    char recv_buf[1024];
    for (size_t i = 0; i < sizeof(recv_buf); ++i) {
        recv_buf[i] = 0;
    }
    const int64_t bytes_recv = fipsos::sys_recvfrom(sock_fd, recv_buf, sizeof(recv_buf) - 1, 0);

    // 5. Cerrar socket TCP (SYS_CLOSE 3)
    fipsos::sys_close(static_cast<uint64_t>(sock_fd));

    if (bytes_recv <= 0) {
        set_cstr(g_ui.browser_status, sizeof(g_ui.browser_status), "Error: Flujo TCP vacio");
        set_cstr(g_ui.last_action, sizeof(g_ui.last_action), "Red: Sin datos recibidos");
        refresh_kernel_telemetry();
        return;
    }

    // Extraer payload HTTP (después de \r\n\r\n)
    const char* body = recv_buf;
    size_t body_len = static_cast<size_t>(bytes_recv);
    for (size_t i = 0; i + 3 < static_cast<size_t>(bytes_recv); ++i) {
        if (recv_buf[i] == '\r' && recv_buf[i+1] == '\n' && recv_buf[i+2] == '\r' && recv_buf[i+3] == '\n') {
            body = recv_buf + i + 4;
            body_len = static_cast<size_t>(bytes_recv) - (i + 4);
            break;
        }
    }

    // Guardar muestra para visualizacion en ventana
    const size_t copy_limit = (body_len < sizeof(g_ui.browser_downloaded_body) - 1) ? body_len : sizeof(g_ui.browser_downloaded_body) - 1;
    for (size_t i = 0; i < copy_limit; ++i) {
        g_ui.browser_downloaded_body[i] = body[i];
    }
    g_ui.browser_downloaded_body[copy_limit] = '\0';

    // 6. Escribir archivo directamente en particion /disk/ mediante sys_open y sys_write
    const int64_t disk_fd = fipsos::sys_open(dest_file, fipsos::O_CREAT | fipsos::O_RDWR);
    if (disk_fd >= 0) {
        fipsos::sys_write(static_cast<uint64_t>(disk_fd), body, body_len);
        fipsos::sys_close(static_cast<uint64_t>(disk_fd));
    }

    push_spark_sample(46);

    size_t p = 0;
    p = fmt_str(g_ui.fmt, p, "TCP Socket: ");
    p = fmt_str(g_ui.fmt, p, dest_file);
    p = fmt_str(g_ui.fmt, p, " guardado (");
    p = fmt_u32(g_ui.fmt, p, static_cast<uint32_t>(body_len));
    p = fmt_str(g_ui.fmt, p, " B)");
    set_cstr(g_ui.last_action, sizeof(g_ui.last_action), g_ui.fmt);

    size_t sp = 0;
    sp = fmt_str(g_ui.browser_status, sp, "200 OK | ");
    sp = fmt_u32(g_ui.browser_status, sp, static_cast<uint32_t>(body_len));
    sp = fmt_str(g_ui.browser_status, sp, " B TCP DMA -> SATA");

    refresh_kernel_telemetry();
}

void action_cycle_bookmark() noexcept {
    g_ui.browser_bookmark = (g_ui.browser_bookmark % 3) + 1;
    if (g_ui.browser_bookmark == 1) {
        set_cstr(g_ui.browser_url, sizeof(g_ui.browser_url), "http://10.0.2.2:8080/host_test.txt");
    } else if (g_ui.browser_bookmark == 2) {
        set_cstr(g_ui.browser_url, sizeof(g_ui.browser_url), "http://10.0.2.2:8080/index.html");
    } else {
        set_cstr(g_ui.browser_url, sizeof(g_ui.browser_url), "http://10.0.2.2:8080/ai_model_v2.bin");
    }
    set_cstr(g_ui.browser_status, sizeof(g_ui.browser_status), "Listo para GET TCP");
}

void action_delete_sata_file() noexcept {
    const char* path_to_del = (g_ui.selected_disk_file == 0) ? "/disk/persistent_note.txt" : "/disk/index.html";
    g_ui.selected_disk_file = (g_ui.selected_disk_file + 1) % 2;
    fipsos::sys_unlink(path_to_del);
    push_spark_sample(16);
    size_t p = 0;
    p = fmt_str(g_ui.fmt, p, "SATA: ");
    p = fmt_str(g_ui.fmt, p, path_to_del);
    p = fmt_str(g_ui.fmt, p, " eliminado (sys_unlink)");
    set_cstr(g_ui.last_action, sizeof(g_ui.last_action), g_ui.fmt);
    refresh_kernel_telemetry();
}

void flip_full() noexcept;
void compose_all_to_backbuf() noexcept;

struct AppCenterItem {
    const char* name;
    const char* desc;
    const char* url_path;
    const char* dest_path;
};

static const AppCenterItem kAppCenterItems[3] = {
    { "sample_tool.elf",   "Diagnostico del sistema y llamadas POSIX", "/sample_tool.elf",   "/disk/bin/sample_tool.elf" },
    { "benchmark_simd.elf", "Calculo vectorial FP32 SSE2 (128 bits)",   "/benchmark_simd.elf", "/disk/bin/benchmark_simd.elf" },
    { "editor_kilo.elf",    "Editor de texto modal en consola TTY",    "/editor_kilo.elf",    "/disk/bin/editor_kilo.elf" },
};

void action_appcenter_cycle() noexcept {
    g_ui.appcenter_selected = (g_ui.appcenter_selected + 1) % 3;
    size_t cp = 0;
    cp = fmt_str(g_ui.appcenter_console, cp, "Seleccionado: ");
    cp = fmt_str(g_ui.appcenter_console, cp, kAppCenterItems[g_ui.appcenter_selected].name);
    set_cstr(g_ui.last_action, sizeof(g_ui.last_action), g_ui.appcenter_console);
    refresh_kernel_telemetry();
}

void action_launch_elf(const char* elf_path, const char* label) noexcept;

void action_appcenter_launch() noexcept {
    const int idx = g_ui.appcenter_selected;
    if (idx < 0 || idx >= 3) return;
    const AppCenterItem& app = kAppCenterItems[idx];

    size_t cp = 0;
    cp = fmt_str(g_ui.appcenter_console, cp, "[Lanzando] ");
    cp = fmt_str(g_ui.appcenter_console, cp, app.dest_path);
    cp = fmt_str(g_ui.appcenter_console, cp, " via sys_execve...");
    set_cstr(g_ui.last_action, sizeof(g_ui.last_action), g_ui.appcenter_console);
    set_cstr(g_ui.appcenter_last_exec, sizeof(g_ui.appcenter_last_exec), app.name);

    push_spark_sample(40);
    refresh_kernel_telemetry();

    action_launch_elf(app.dest_path, app.name);

    cp = 0;
    cp = fmt_str(g_ui.appcenter_console, cp, "[Ejecutado] ");
    cp = fmt_str(g_ui.appcenter_console, cp, app.name);
    cp = fmt_str(g_ui.appcenter_console, cp, " finalizo correctamente");
    set_cstr(g_ui.last_action, sizeof(g_ui.last_action), g_ui.appcenter_console);
    refresh_kernel_telemetry();
}

void action_appcenter_install() noexcept {
    const int idx = g_ui.appcenter_selected;
    if (idx < 0 || idx >= 3) return;
    const AppCenterItem& app = kAppCenterItems[idx];

    // 1. Crear socket POSIX
    const int64_t sock_fd = fipsos::sys_socket(fipsos::AF_INET, fipsos::SOCK_STREAM, 0);
    if (sock_fd < 0) {
        set_cstr(g_ui.appcenter_console, sizeof(g_ui.appcenter_console), "Error: socket() fallo");
        set_cstr(g_ui.last_action, sizeof(g_ui.last_action), "AppCenter: socket() error");
        refresh_kernel_telemetry();
        return;
    }

    // 2. Conectar al host 10.0.2.2:8080
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
        set_cstr(g_ui.last_action, sizeof(g_ui.last_action), "AppCenter: Error conexion TCP");
        refresh_kernel_telemetry();
        return;
    }

    // 3. Peticion HTTP GET
    char req_buf[256];
    size_t req_len = 0;
    req_len = fmt_str(req_buf, req_len, "GET ");
    req_len = fmt_str(req_buf, req_len, app.url_path);
    req_len = fmt_str(req_buf, req_len, " HTTP/1.0\r\nHost: 10.0.2.2:8080\r\nUser-Agent: FipSOs-AppCenter/2.4\r\nConnection: close\r\n\r\n");
    fipsos::sys_sendto(sock_fd, req_buf, req_len, 0);

    // 4. Abrir archivo de destino en /disk/bin/<app>
    const int64_t disk_fd = fipsos::sys_open(app.dest_path, fipsos::O_CREAT | fipsos::O_RDWR);
    if (disk_fd < 0) {
        fipsos::sys_close(static_cast<uint64_t>(sock_fd));
        set_cstr(g_ui.appcenter_console, sizeof(g_ui.appcenter_console), "Error: No se pudo crear en /disk/bin/");
        set_cstr(g_ui.last_action, sizeof(g_ui.last_action), "AppCenter: Error abrir archivo");
        refresh_kernel_telemetry();
        return;
    }

    // 5. Descargar datos y escribir a SATA DiskFS
    char recv_buf[2048];
    int64_t n = fipsos::sys_recvfrom(sock_fd, recv_buf, sizeof(recv_buf), 0);
    size_t total_written = 0;
    if (n > 0) {
        const char* body = recv_buf;
        size_t body_len = static_cast<size_t>(n);
        for (size_t i = 0; i + 3 < static_cast<size_t>(n); ++i) {
            if (recv_buf[i] == '\r' && recv_buf[i+1] == '\n' && recv_buf[i+2] == '\r' && recv_buf[i+3] == '\n') {
                body = recv_buf + i + 4;
                body_len = static_cast<size_t>(n) - (i + 4);
                break;
            }
        }
        if (body_len > 0) {
            fipsos::sys_write(static_cast<uint64_t>(disk_fd), body, body_len);
            total_written += body_len;
        }
    }

    while (true) {
        n = fipsos::sys_recvfrom(sock_fd, recv_buf, sizeof(recv_buf), 0);
        if (n <= 0) break;
        fipsos::sys_write(static_cast<uint64_t>(disk_fd), recv_buf, static_cast<size_t>(n));
        total_written += static_cast<size_t>(n);
    }

    fipsos::sys_close(static_cast<uint64_t>(disk_fd));
    fipsos::sys_close(static_cast<uint64_t>(sock_fd));

    // Sincronizar persistentemente a disco SATA AHCI
    if (g_ui.fb_fd >= 0) {
        fipsos::sys_ioctl(static_cast<uint64_t>(g_ui.fb_fd), fipsos::DISK_IOCTL_SAVE_NOTE, 0ULL);
    }

    g_ui.appcenter_installed[idx] = true;
    size_t cp = 0;
    cp = fmt_str(g_ui.appcenter_console, cp, "[TCP OK] ");
    cp = fmt_str(g_ui.appcenter_console, cp, app.name);
    cp = fmt_str(g_ui.appcenter_console, cp, " (");
    cp = fmt_u32(g_ui.appcenter_console, cp, static_cast<uint32_t>(total_written));
    cp = fmt_str(g_ui.appcenter_console, cp, " B en /disk/bin/)");

    set_cstr(g_ui.last_action, sizeof(g_ui.last_action), g_ui.appcenter_console);
    push_spark_sample(48);
    refresh_kernel_telemetry();
}

void action_launch_elf(const char* elf_path, const char* label) noexcept {
    g_ui.start_open = false;
    const int64_t rc = fipsos::sys_execve(elf_path);
    // Re-enable Desktop Compositor mode in case child wrote to TTY or /dev/fb0 widget
    fipsos::sys_ioctl(static_cast<uint64_t>(g_ui.fb_fd), fipsos::FB_IOCTL_SET_DESKTOP_MODE, 1ULL);
    g_ui.app_launches += 1U;
    push_spark_sample(38);
    refresh_kernel_telemetry();

    size_t p = 0;
    p = fmt_str(g_ui.fmt, p, label);
    p = fmt_str(g_ui.fmt, p, " -> exit(");
    p = fmt_u32(g_ui.fmt, p, (rc >= 0) ? static_cast<uint32_t>(rc) : 99U);
    p = fmt_str(g_ui.fmt, p, ") OK [#");
    p = fmt_u32(g_ui.fmt, p, g_ui.app_launches);
    p = fmt_str(g_ui.fmt, p, "]");
    set_cstr(g_ui.last_action, sizeof(g_ui.last_action), g_ui.fmt);

    compose_all_to_backbuf();
    flip_full();
}

// --- Desktop Shortcuts, Floating Windows, Start Menu & Centered Taskbar ---

void draw_desktop_shortcuts() noexcept {
    // Top-right Zero-Bloat + Hardware CR3 Status Pill
    const int pill_w = 372;
    const int pill_x = g_ui.screen_w - pill_w - 16;
    blend_round_rect(pill_x, 12, pill_w, 30, 6, 0x0B1628, 204);
    stroke_round_rect(pill_x, 12, pill_w, 30, 6, 0x26466D);

    const uint32_t used_mib  = g_ui.sys_tel.ram_used_kib / 1024U;
    const uint32_t total_mib = (g_ui.sys_tel.ram_total_kib > 0U) ? (g_ui.sys_tel.ram_total_kib / 1024U) : 504U;
    size_t p = 0;
    p = fmt_str(g_ui.fmt, p, "FipSOs v3.1 | CR3 ");
    p = fmt_hex_u64(g_ui.fmt, p, g_ui.sys_tel.active_cr3);
    p = fmt_str(g_ui.fmt, p, " | ");
    p = fmt_u32(g_ui.fmt, p, used_mib);
    p = fmt_str(g_ui.fmt, p, "/");
    p = fmt_u32(g_ui.fmt, p, total_mib);
    p = fmt_str(g_ui.fmt, p, "M");
    draw_text(g_ui.fmt, pill_x + 14, 23, 0x7AE582);

    // 4 Left Desktop Shortcut Tiles
    struct Shortcut {
        int         y;
        const char* label;
        int         kind;
    };
    constexpr Shortcut kShortcuts[4] = {
        {20,  "Mi FipSOs", 2},
        {108, "Red & SIMD", 0},
        {196, "Esfera 3D", 1},
        {284, "App Rust",  3},
    };

    for (int i = 0; i < 4; ++i) {
        const int sx = 20;
        const int sy = kShortcuts[i].y;
        const bool hov = pt_in_rect(g_ui.mouse_x, g_ui.mouse_y, sx, sy, 84, 76);
        if (hov) {
            blend_round_rect(sx, sy, 84, 76, 6, 0x38BDF8, 52);
            stroke_round_rect(sx, sy, 84, 76, 6, 0x38BDF8);
        }
        if (kShortcuts[i].kind == 2) {
            draw_icon_folder(sx + 32, sy + 14);
        } else if (kShortcuts[i].kind == 0) {
            draw_icon_pulse(sx + 32, sy + 14);
        } else if (kShortcuts[i].kind == 1) {
            draw_icon_orb3d(sx + 32, sy + 14);
        } else {
            draw_icon_rust_elf(sx + 32, sy + 14);
        }
        draw_text_centered(kShortcuts[i].label, sx, sy + 46, 84, 16, 0xE2E8F0);
    }
}

void draw_window_chrome(const FloatingWindow& w, bool focused) noexcept {
    draw_shadow(w.x, w.y, w.w, w.h);

    // Frosted Mica window body
    blend_round_rect(w.x, w.y, w.w, w.h, 8, 0x0D1829, 238);

    // Titlebar strip
    const uint32_t title_bg = focused ? 0x152742 : 0x101E33;
    fill_round_rect(w.x + 1, w.y + 1, w.w - 2, 32, 7, title_bg);
    fill_rect(w.x + 1, w.y + 25, w.w - 2, 8, title_bg);
    fill_rect(w.x + 1, w.y + 33, w.w - 2, 1, focused ? 0x264B73 : 0x1B314C);

    const uint32_t border_col = focused ? w.accent_rgb : 0x2B4263;
    stroke_round_rect(w.x, w.y, w.w, w.h, 8, border_col);

    // Window accent dot + title
    fill_round_rect(w.x + 12, w.y + 11, 10, 10, 3, w.accent_rgb);
    draw_text(w.title, w.x + 28, w.y + 13, focused ? 0xF8FAFC : 0x94A3B8);

    // Minimize [-] button
    const int min_x = w.x + w.w - 58;
    const int btn_y = w.y + 6;
    const bool hov_min = pt_in_rect(g_ui.mouse_x, g_ui.mouse_y, min_x, btn_y, 24, 20);
    fill_round_rect(min_x, btn_y, 24, 20, 4, hov_min ? 0x284366 : 0x1A2E49);
    fill_rect(min_x + 7, btn_y + 10, 10, 2, 0xE2E8F0);

    // Close [X] button
    const int cls_x = w.x + w.w - 30;
    const bool hov_cls = pt_in_rect(g_ui.mouse_x, g_ui.mouse_y, cls_x, btn_y, 24, 20);
    fill_round_rect(cls_x, btn_y, 24, 20, 4, hov_cls ? 0xDC2626 : 0x2A1E2E);
    draw_char('X', cls_x + 8, btn_y + 6, 0xF8FAFC);
}

void draw_win_monitor(const FloatingWindow& w, bool focused) noexcept {
    draw_window_chrome(w, focused);
    const int cx = w.x + 14;
    const int cy = w.y + 42;
    const int clip_r = w.x + w.w - 14;

    // Card 1: Procesos Round-Robin SMP Cores 0 & 1
    fill_round_rect(cx, cy, w.w - 28, 96, 6, 0x091220);
    stroke_round_rect(cx, cy, w.w - 28, 96, 6, 0x1E3A5F);
    draw_text("Planificacion Distribuida SMP (Cores 0 y 1):", cx + 10, cy + 8, 0x38BDF8, clip_r);

    size_t p = 0;
    p = fmt_str(g_ui.fmt, p, "[CPU0] BSP: desktop_ui.elf (CR3=");
    p = fmt_hex_u64(g_ui.fmt, p, g_ui.sys_tel.active_cr3);
    p = fmt_str(g_ui.fmt, p, ")");
    draw_text(g_ui.fmt, cx + 10, cy + 24, 0x93C5FD, clip_r);

    p = 0;
    p = fmt_str(g_ui.fmt, p, "[CPU1] AP : ");
    if (g_ui.sys_tel.bg_task_state == 1) {
        p = fmt_str(g_ui.fmt, p, "bg_downloader PID 1051 (Activo DMA)");
    } else if (g_ui.sys_tel.bg_task_state == 2) {
        p = fmt_str(g_ui.fmt, p, "simd-worker Detenido (SIGKILL 9)");
    } else {
        p = fmt_str(g_ui.fmt, p, "simd-worker PID 1035 (Activo CPL=0)");
    }
    draw_text(g_ui.fmt, cx + 10, cy + 40, (g_ui.sys_tel.bg_task_state == 2) ? 0xF87171 : 0x00F5D4, clip_r);

    p = 0;
    p = fmt_str(g_ui.fmt, p, "[TAREAS] Shell PID 1033 | SIMD PID 1037 | UI PID 1041");
    draw_text(g_ui.fmt, cx + 10, cy + 56, 0xE2E8F0, clip_r);

    p = 0;
    p = fmt_str(g_ui.fmt, p, "[BGT] PID 1051 | bg_downloader | ");
    p = fmt_str(g_ui.fmt, p, (g_ui.sys_tel.bg_task_state == 1) ? "Activo (DMA Core 1)" : "Inactivo / Detenido");
    draw_text(g_ui.fmt, cx + 10, cy + 72, (g_ui.sys_tel.bg_task_state == 1) ? 0x4ADE80 : 0x94A3B8, clip_r);

    // Card 2: Hardware Sensors (CPU MHz, Temp, Cores/Threads, Fault Recovery)
    const int c2_y = cy + 102;
    fill_round_rect(cx, c2_y, w.w - 28, 48, 6, 0x091220);
    stroke_round_rect(cx, c2_y, w.w - 28, 48, 6, 0x1E3A5F);

    p = 0;
    p = fmt_str(g_ui.fmt, p, "CPU: ");
    p = fmt_u32(g_ui.fmt, p, g_ui.sys_tel.cpu_mhz);
    p = fmt_str(g_ui.fmt, p, " MHz | Cores: ");
    p = fmt_u32(g_ui.fmt, p, static_cast<uint32_t>(g_ui.sys_tel.physical_cores));
    p = fmt_str(g_ui.fmt, p, "C/");
    p = fmt_u32(g_ui.fmt, p, static_cast<uint32_t>(g_ui.sys_tel.logical_threads));
    p = fmt_str(g_ui.fmt, p, "T | Temp: ");
    if (g_ui.sys_tel.temp_celsius == 0U) {
        p = fmt_str(g_ui.fmt, p, "N/A (VM Hypervisor)");
    } else {
        p = fmt_u32(g_ui.fmt, p, g_ui.sys_tel.temp_celsius);
        p = fmt_str(g_ui.fmt, p, " C (DTS)");
    }
    draw_text(g_ui.fmt, cx + 10, c2_y + 8, 0x38BDF8, clip_r);

    p = 0;
    p = fmt_str(g_ui.fmt, p, "Recuperacion #PF/#GP: ");
    p = fmt_u32(g_ui.fmt, p, static_cast<uint32_t>(g_ui.sys_tel.user_faults_recovered));
    p = fmt_str(g_ui.fmt, p, " fallos | Sw: ");
    p = fmt_u32(g_ui.fmt, p, g_ui.sys_tel.daemon_switches);
    p = fmt_str(g_ui.fmt, p, " | Preemp: ");
    p = fmt_u32(g_ui.fmt, p, g_ui.sys_tel.preemptions);
    draw_text(g_ui.fmt, cx + 10, c2_y + 26, 0x7AE582, clip_r);

    // Card 3: Red PCIe Intel e1000e (eth0 Gigabit Ethernet)
    const int c3_y = c2_y + 54;
    fill_round_rect(cx, c3_y, w.w - 28, 54, 6, 0x091220);
    stroke_round_rect(cx, c3_y, w.w - 28, 54, 6, 0x1E3A5F);

    p = 0;
    p = fmt_str(g_ui.fmt, p, "PCIe NIC: e1000e 00:03.0 | Link: ");
    p = fmt_str(g_ui.fmt, p, (g_ui.sys_tel.net_link_up != 0) ? "UP (1 Gbps)" : "DOWN");
    p = fmt_str(g_ui.fmt, p, " | MAC: ");
    for (int mi = 0; mi < 6; ++mi) {
        p = fmt_hex_byte(g_ui.fmt, p, g_ui.sys_tel.mac[mi]);
        if (mi < 5) p = fmt_str(g_ui.fmt, p, ":");
    }
    draw_text(g_ui.fmt, cx + 10, c3_y + 8, (g_ui.sys_tel.net_link_up != 0) ? 0x00F5D4 : 0xEF4444, clip_r);

    p = 0;
    p = fmt_str(g_ui.fmt, p, "Iface: eth0 (/dev/net/eth0) | TX: ");
    p = fmt_u32(g_ui.fmt, p, g_ui.sys_tel.net_tx_packets);
    p = fmt_str(g_ui.fmt, p, " | RX: ");
    p = fmt_u32(g_ui.fmt, p, g_ui.sys_tel.net_rx_packets);
    draw_text(g_ui.fmt, cx + 10, c3_y + 26, 0x38BDF8, clip_r);

    // Card 4: RAM Bar & SATA AHCI
    const int c4_y = c3_y + 60;
    fill_round_rect(cx, c4_y, w.w - 28, 48, 6, 0x091220);
    stroke_round_rect(cx, c4_y, w.w - 28, 48, 6, 0x1E3A5F);

    const uint32_t used_kib  = g_ui.sys_tel.ram_used_kib;
    const uint32_t total_kib = (g_ui.sys_tel.ram_total_kib > 0U) ? g_ui.sys_tel.ram_total_kib : (504U * 1024U);
    const uint32_t pct       = (used_kib * 100U) / total_kib;

    p = 0;
    p = fmt_str(g_ui.fmt, p, "RAM: ");
    p = fmt_u32(g_ui.fmt, p, used_kib / 1024U);
    p = fmt_str(g_ui.fmt, p, "/");
    p = fmt_u32(g_ui.fmt, p, total_kib / 1024U);
    p = fmt_str(g_ui.fmt, p, " MiB (");
    p = fmt_u32(g_ui.fmt, p, pct);
    p = fmt_str(g_ui.fmt, p, "%) | SATA: ");
    p = fmt_u32(g_ui.fmt, p, static_cast<uint32_t>(g_ui.sys_tel.disk_sectors_read));
    p = fmt_str(g_ui.fmt, p, "R/");
    p = fmt_u32(g_ui.fmt, p, static_cast<uint32_t>(g_ui.sys_tel.disk_sectors_written));
    p = fmt_str(g_ui.fmt, p, "W");
    draw_text(g_ui.fmt, cx + 10, c4_y + 8, 0x7AE582, clip_r);

    const int bar_w  = w.w - 48;
    const int fill_w = max_i(12, static_cast<int>((static_cast<uint64_t>(bar_w) * used_kib) / total_kib));
    fill_round_rect(cx + 10, c4_y + 26, bar_w, 12, 4, 0x16243B);
    fill_round_rect(cx + 10, c4_y + 26, min_i(bar_w, fill_w), 12, 4, 0x10B981);

    // 3 Action Buttons
    const int by = w.y + w.h - 36;
    draw_button(w.x + 14,  by, 145, 26, "Lanzar BG Task", 0x123254, 0x38BDF8, 0xF8FAFC);
    draw_button(w.x + 170, by, 145, 26, "Kill Task/Dmn",  0x4A1520, 0xEF4444, 0xF8FAFC);
    draw_button(w.x + 326, by, 150, 26, "Refrescar Red",  0x123B34, 0x10B981, 0xF8FAFC);
}

void draw_win_browser(const FloatingWindow& w, bool focused) noexcept {
    draw_window_chrome(w, focused);
    const int cx = w.x + 14;
    const int cy = w.y + 42;

    // URL Address Bar Pill
    fill_round_rect(cx, cy, w.w - 28, 28, 5, 0x081120);
    stroke_round_rect(cx, cy, w.w - 28, 28, 5, 0x28476E);

    size_t p = 0;
    p = fmt_str(g_ui.fmt, p, "URL: ");
    p = fmt_str(g_ui.fmt, p, g_ui.browser_url);
    p = fmt_str(g_ui.fmt, p, "  [#");
    p = fmt_u32(g_ui.fmt, p, static_cast<uint32_t>(g_ui.browser_bookmark));
    p = fmt_str(g_ui.fmt, p, "/3]");
    draw_text(g_ui.fmt, cx + 10, cy + 9, 0x38BDF8);

    // Web Page Content Frame (HTML Viewer)
    const int fy = cy + 34;
    const int fh = 186;
    fill_round_rect(cx, fy, w.w - 28, fh, 6, 0x07111D);
    stroke_round_rect(cx, fy, w.w - 28, fh, 6, 0x1E3A5F);

    draw_text("=== NAVEGADOR WEB / DESCARGAS TCP (FipSOs v3.1) ===", cx + 12, fy + 10, 0x00F5D4);
    draw_text("URL: http://10.0.2.2:8080/host_test.txt (Acceso Rapido Host WSL2)", cx + 12, fy + 26, 0xFBBF24);

    if (g_ui.browser_downloaded_body[0] != '\0') {
        draw_text("[TCP STREAM] Datos recibidos por socket POSIX via e1000e DMA:", cx + 12, fy + 48, 0x38BDF8);
        draw_text(g_ui.browser_downloaded_body, cx + 12, fy + 68, 0x7AE582);
        draw_text("Syscalls: socket(41), connect(42), sendto(44), recvfrom(45) OK", cx + 12, fy + 112, 0x67E8F9);
        draw_text("Persistencia en SATA AHCI: /disk/host_test.txt (sys_write OK)", cx + 12, fy + 130, 0xE2E8F0);
    } else if (g_ui.browser_bookmark == 1) {
        draw_text("[TCP] Acceso directo al Host HTTP en 10.0.2.2:8080/host_test.txt", cx + 12, fy + 48, 0xE2E8F0);
        draw_text("Pulsar [Descargar a SATA] para conectar socket TCP por e1000e DMA.", cx + 12, fy + 64, 0x93C5FD);
        draw_text("Pila TCP/IP: Ethernet II -> ARP -> IPv4 -> TCP Three-Way Handshake", cx + 12, fy + 80, 0x67E8F9);
        draw_text("Planificacion SMP: Core 1 (AP) atiende transferencias concurrentes.", cx + 12, fy + 96, 0x4ADE80);
        draw_text("Control de Tareas sys_kill (62) + Borrado en Disco sys_unlink (87)", cx + 12, fy + 112, 0xF472B6);
        draw_text("Ruta guardada en disco: /disk/host_test.txt (SATA AHCI DMA)", cx + 12, fy + 130, 0xCBD5E1);
    } else if (g_ui.browser_bookmark == 2) {
        draw_text("[HTML] index.html (Portal Oficial FipSOs v3.1 - Arquitectura Abierta)", cx + 12, fy + 48, 0xE2E8F0);
        draw_text("Acelerador C11 nativo bajo sandbox FfiJail con memoria DMA.", cx + 12, fy + 64, 0xFDBA74);
        draw_text("Descargas HTTP directas por DMA PCIe e1000e al disco SATA AHCI.", cx + 12, fy + 80, 0x67E8F9);
        draw_text("Persistencia en disco SATA LBA 20..21 verificada con checksum.", cx + 12, fy + 96, 0x4ADE80);
        draw_text("Ruta guardada en disco: /disk/index.html (256 B)", cx + 12, fy + 112, 0xCBD5E1);
    } else {
        draw_text("[MODEL] ai_model_v2.bin (Pesos Tensoriales FP32 SSE2)", cx + 12, fy + 48, 0xE2E8F0);
        draw_text("Paquete binario para despliegue en caliente y actualizacion segura.", cx + 12, fy + 64, 0xC4B5FD);
        draw_text("Aislamiento de procesos por arboles PML4 independientes (CR3).", cx + 12, fy + 80, 0x67E8F9);
        draw_text("Supervivencia del Kernel ante fallos #PF/#GP en Ring 3 verificada.", cx + 12, fy + 96, 0x4ADE80);
        draw_text("Ruta guardada en disco: /disk/ai_model_v2.bin (1024 B)", cx + 12, fy + 112, 0xCBD5E1);
    }

    // Status strip
    fill_round_rect(cx, fy + fh + 8, w.w - 28, 26, 5, 0x0F243C);
    draw_text("Estado TCP:", cx + 8, fy + fh + 16, 0x38BDF8);
    draw_text(g_ui.browser_status, cx + 110, fy + fh + 16, 0x4ADE80);

    // 3 Action Buttons
    const int by = w.y + w.h - 36;
    draw_button(w.x + 14,  by, 150, 26, "Ir / GET HTTP",    0x064E3B, 0x34D399, 0xF8FAFC);
    draw_button(w.x + 174, by, 150, 26, "Descargar a SATA", 0x1E3A8A, 0x60A5FA, 0xF8FAFC);
    draw_button(w.x + 334, by, 150, 26, "Sig. Marcador",    0x4C1D95, 0xC084FC, 0xF8FAFC);
    draw_button(w.x + 334, by, 150, 26, "Sig. Marcador",  0x4C1D95, 0xC084FC, 0xF8FAFC);
}

void draw_win_appcenter(const FloatingWindow& w, bool focused) noexcept {
    draw_window_chrome(w, focused);
    const int cx = w.x + 14;
    const int cy = w.y + 40;

    // Header Repository Banner
    fill_round_rect(cx, cy, w.w - 28, 24, 4, 0x0F1E36);
    stroke_round_rect(cx, cy, w.w - 28, 24, 4, 0x2563EB);
    draw_text("Repositorio FipSOs Host: http://10.0.2.2:8080/ [TCP Sockets]", cx + 8, cy + 8, 0x60A5FA);

    // 3 Application Cards
    for (int i = 0; i < 3; ++i) {
        const int card_y = cy + 28 + i * 42;
        const bool is_sel = (i == g_ui.appcenter_selected);
        const uint32_t bg_col = is_sel ? 0x172554 : 0x0B1324;
        const uint32_t border_col = is_sel ? 0xFBBF24 : 0x1E3A5F;

        fill_round_rect(cx, card_y, w.w - 28, 38, 5, bg_col);
        stroke_round_rect(cx, card_y, w.w - 28, 38, 5, border_col);

        if (is_sel) {
            fill_round_rect(cx + 6, card_y + 11, 5, 16, 2, 0xFBBF24);
        }

        draw_text(kAppCenterItems[i].name, cx + 16, card_y + 6, is_sel ? 0xFBBF24 : 0x38BDF8);
        draw_text(kAppCenterItems[i].desc, cx + 16, card_y + 22, 0x94A3B8);

        // Status badge
        const bool installed = g_ui.appcenter_installed[i];
        const int badge_x = cx + w.w - 28 - 110;
        const int badge_y = card_y + 8;
        if (installed) {
            fill_round_rect(badge_x, badge_y, 100, 22, 4, 0x064E3B);
            stroke_round_rect(badge_x, badge_y, 100, 22, 4, 0x34D399);
            draw_text("INSTALADO", badge_x + 12, badge_y + 7, 0x34D399);
        } else {
            fill_round_rect(badge_x, badge_y, 100, 22, 4, 0x451A03);
            stroke_round_rect(badge_x, badge_y, 100, 22, 4, 0xF59E0B);
            draw_text("DISPONIBLE", badge_x + 8, badge_y + 7, 0xFBBF24);
        }
    }

    // Disk & System Info Row
    const int inf_y = cy + 158;
    fill_round_rect(cx, inf_y, w.w - 28, 38, 5, 0x091220);
    stroke_round_rect(cx, inf_y, w.w - 28, 38, 5, 0x1E3A5F);

    size_t p = 0;
    p = fmt_str(g_ui.fmt, p, "SATA AHCI: /disk/bin/ (");
    p = fmt_u32(g_ui.fmt, p, static_cast<uint32_t>(g_ui.sys_tel.disk_files_count));
    p = fmt_str(g_ui.fmt, p, " archivos) | Ultima ejecucion: ");
    p = fmt_str(g_ui.fmt, p, g_ui.appcenter_last_exec);
    draw_text(g_ui.fmt, cx + 8, inf_y + 6, 0xE2E8F0);

    p = 0;
    p = fmt_str(g_ui.fmt, p, "CR3: ");
    p = fmt_hex_u64(g_ui.fmt, p, g_ui.sys_tel.active_cr3);
    p = fmt_str(g_ui.fmt, p, " | SMP APIC: 2 Cores | ACPI HLT: ACTIVO");
    draw_text(g_ui.fmt, cx + 8, inf_y + 22, 0x38BDF8);

    // Execution & Console Box
    const int con_y = inf_y + 42;
    fill_round_rect(cx, con_y, w.w - 28, 34, 5, 0x080E1A);
    stroke_round_rect(cx, con_y, w.w - 28, 34, 5, 0x334155);
    draw_text("Consola:", cx + 8, con_y + 5, 0x38BDF8);
    draw_text(g_ui.appcenter_console, cx + 64, con_y + 5, 0xF8FAFC);

    // 4 Action Buttons at bottom
    const int by = w.y + w.h - 36;
    draw_button(w.x + 14,  by, 160, 26, "Instalar via TCP", 0x1E3A8A, 0x38BDF8, 0xF8FAFC);
    draw_button(w.x + 184, by, 140, 26, "Lanzar App",       0x064E3B, 0x34D399, 0xF8FAFC);
    draw_button(w.x + 334, by, 130, 26, "Ciclar App",       0x4C1D95, 0xC084FC, 0xF8FAFC);
    draw_button(w.x + 474, by, 112, 26, "Guardar Dsk",      0x1B2A4A, 0xFBBF24, 0xF8FAFC);
}

void draw_win_sphere(const FloatingWindow& w, bool focused) noexcept {
    draw_window_chrome(w, focused);
    const int sx = w.x + 16;
    const int sy = w.y + 46;

    // 132x132 3D Phong Sphere Viewport
    stroke_round_rect(sx - 2, sy - 2, kSphereSize + 4, kSphereSize + 4, 4, 0x818CF8);
    for (int py = 0; py < kSphereSize; ++py) {
        for (int px = 0; px < kSphereSize; ++px) {
            put_px(sx + px, sy + py, g_ui.sphere_tile[py * kSphereSize + px]);
        }
    }

    // Right details panel
    const int dx = sx + kSphereSize + 14;
    const int dy = sy;
    draw_text("3D Shader Phong", dx, dy + 4, 0x818CF8);
    draw_text("FP32 IEEE-754", dx, dy + 20, 0x00F5D4);
    draw_text("SIMD: SSE2 sqrtss", dx, dy + 36, 0x38BDF8);

    size_t p = 0;
    p = fmt_str(g_ui.fmt, p, "Yaw: ");
    p = fmt_u32(g_ui.fmt, p, static_cast<uint32_t>((g_ui.yaw_rad + kPi) * 57.29f));
    p = fmt_str(g_ui.fmt, p, " deg");
    draw_text(g_ui.fmt, dx, dy + 56, 0xE2E8F0);

    p = 0;
    p = fmt_str(g_ui.fmt, p, "Pitch: ");
    p = fmt_u32(g_ui.fmt, p, static_cast<uint32_t>((g_ui.pitch_rad + kPi) * 57.29f));
    p = fmt_str(g_ui.fmt, p, " deg");
    draw_text(g_ui.fmt, dx, dy + 72, 0xE2E8F0);

    draw_text("Blinn Spec: ^16", dx, dy + 92, 0xFBBF24);
    draw_text("Luz: [Drag Raton]", dx, dy + 108, 0x94A3B8);

    const char* pal_name = "Aurora Cyan";
    if ((g_ui.palette_mode & 3U) == 1U) pal_name = "Emerald Neon";
    else if ((g_ui.palette_mode & 3U) == 2U) pal_name = "Violet Glow";
    else if ((g_ui.palette_mode & 3U) == 3U) pal_name = "Solar Amber";
    p = 0;
    p = fmt_str(g_ui.fmt, p, "Modo: ");
    p = fmt_str(g_ui.fmt, p, pal_name);
    draw_text(g_ui.fmt, dx, dy + 126, 0xC084FC);

    const int by = w.y + w.h - 36;
    draw_button(w.x + 14,  by, 160, 26, "Rotar 3D (T)",  0x1E1B4B, 0x818CF8, 0xF8FAFC);
    draw_button(w.x + 185, by, 165, 26, "Cambiar Color", 0x311042, 0xC084FC, 0xF8FAFC);
}

void draw_floating_window(int wid, bool focused) noexcept {
    const FloatingWindow& w = g_ui.wins[wid];
    if (!w.open || w.minimized) {
        return;
    }
    if (wid == 0) {
        draw_win_monitor(w, focused);
    } else if (wid == 1) {
        draw_win_sphere(w, focused);
    } else if (wid == 2) {
        draw_win_browser(w, focused);
    } else {
        draw_win_appcenter(w, focused);
    }
}

void draw_start_menu() noexcept {
    if (!g_ui.start_open) {
        return;
    }
    const int sw = 440;
    const int sh = 350;
    const int sx = (g_ui.screen_w - sw) / 2;
    const int sy = g_ui.taskbar_y - sh - 8;

    draw_shadow(sx, sy, sw, sh);
    blend_round_rect(sx, sy, sw, sh, 10, 0x0E1B30, 244);
    stroke_round_rect(sx, sy, sw, sh, 10, 0x38BDF8);

    // Search bar pill
    fill_round_rect(sx + 20, sy + 16, sw - 40, 30, 6, 0x081120);
    stroke_round_rect(sx + 20, sy + 16, sw - 40, 30, 6, 0x28476E);
    draw_text("> Buscar apps ELF64, VMO, eth0 o SIMD...", sx + 32, sy + 27, 0x94A3B8);

    draw_text("Anclado - Lanzador de Aplicaciones FipSOs v3.1", sx + 22, sy + 58, 0xE2E8F0);

    struct AppTile {
        const char* title;
        const char* sub;
        int         icon_kind;
    };
    constexpr AppTile kTiles[6] = {
        {"Admin Tareas", "Procesos & CPU",  0},
        {"Navegador",    "Web & Descargas", 1},
        {"App Center",   "TCP Software Dsk", 2},
        {"npu_infer",    "Rust ELF64",      3},
        {"avatar_cpp",   "C++20 ELF64",     1},
        {"Terminal TTY", "Consola Shell",   0},
    };

    for (int i = 0; i < 6; ++i) {
        const int col = i % 3;
        const int row = i / 3;
        const int tx  = sx + 20 + col * 140;
        const int ty  = sy + 78 + row * 96;
        const int tw  = 120;
        const int th  = 84;
        const bool hov = pt_in_rect(g_ui.mouse_x, g_ui.mouse_y, tx, ty, tw, th);

        fill_round_rect(tx, ty, tw, th, 7, hov ? 0x1D3557 : 0x13233B);
        stroke_round_rect(tx, ty, tw, th, 7, hov ? 0x00F5D4 : 0x243E60);

        if (kTiles[i].icon_kind == 0) {
            draw_icon_pulse(tx + 50, ty + 12);
        } else if (kTiles[i].icon_kind == 1) {
            draw_icon_orb3d(tx + 50, ty + 12);
        } else if (kTiles[i].icon_kind == 2) {
            draw_icon_folder(tx + 50, ty + 12);
        } else {
            draw_icon_rust_elf(tx + 50, ty + 12);
        }

        draw_text_centered(kTiles[i].title, tx, ty + 42, tw, 14, 0xF8FAFC);
        draw_text_centered(kTiles[i].sub,   tx, ty + 60, tw, 12, 0x94A3B8);
    }

    // Bottom profile, Fault test, Exit to TTY & ACPI Poweroff footer
    fill_rect(sx + 1, sy + sh - 52, sw - 2, 1, 0x223B5B);
    fill_round_rect(sx + 14, sy + sh - 40, 26, 26, 13, 0x0284C7);
    draw_text("FP", sx + 19, sy + sh - 31, 0xFFFFFF);
    draw_text("v3.1", sx + 45, sy + sh - 31, 0x94A3B8);

    draw_button(sx + 82,       sy + sh - 40, 110, 28, "Probar Fault", 0x4A1520, 0xEF4444, 0xFEE2E2);
    draw_button(sx + 82 + 116, sy + sh - 40, 110, 28, "Salir a TTY",  0x2D1B4E, 0xA855F7, 0xF3E8FF);
    draw_button(sx + 82 + 232, sy + sh - 40, 110, 28, "Apagar ACPI", 0x3F1D2B, 0xF43F5E, 0xFFE4E6);
}

void draw_taskbar() noexcept {
    // Windows 11 Frosted Dark Mica Translucent Bottom Bar
    blend_rect(0, g_ui.taskbar_y, g_ui.screen_w, g_ui.taskbar_h, 0x08101E, 224);
    fill_rect(0, g_ui.taskbar_y, g_ui.screen_w, 1, 0x2E4F7A);

    // Left System Badge
    fill_round_rect(14, g_ui.taskbar_y + 8, 188, 32, 6, 0x101E34);
    stroke_round_rect(14, g_ui.taskbar_y + 8, 188, 32, 6, 0x223B5C);
    fill_round_rect(24, g_ui.taskbar_y + 19, 8, 8, 4, 0x10B981);
    draw_text("FipSOs v3.1", 38, g_ui.taskbar_y + 20, 0xE2E8F0);

    // Centered Windows 11 Icon Dock (6 buttons)
    constexpr int dock_stride = 48;
    const int dock_x0         = (g_ui.screen_w - (6 * dock_stride)) / 2;
    const int top_win         = g_ui.z_order[3];

    for (int i = 0; i < 6; ++i) {
        const int bx  = dock_x0 + i * dock_stride;
        const int by  = g_ui.taskbar_y + 6;
        const bool hov = pt_in_rect(g_ui.mouse_x, g_ui.mouse_y, bx, by, 40, 36);
        const bool act = (i == 0 && g_ui.start_open) ||
                         (i >= 1 && i <= 4 && g_ui.wins[i - 1].open && !g_ui.wins[i - 1].minimized && top_win == (i - 1));

        if (hov || act) {
            fill_round_rect(bx, by, 40, 36, 6, hov ? 0x20385C : 0x162946);
            stroke_round_rect(bx, by, 40, 36, 6, act ? 0x38BDF8 : 0x2E4F7A);
        }

        if (i == 0) {
            // Windows 11 4-pane Start Emblem
            draw_win11_logo(bx + 20, by + 17, 8, 0x38BDF8, 0x0EA5E9, 0x0284C7, 0x00F5D4);
        } else if (i == 1) {
            draw_icon_pulse(bx + 10, by + 7);
        } else if (i == 2) {
            draw_icon_orb3d(bx + 10, by + 7);
        } else if (i == 3) {
            draw_icon_folder(bx + 10, by + 7);
        } else if (i == 4) {
            draw_icon_rust_elf(bx + 10, by + 7);
        } else {
            draw_icon_tty(bx + 10, by + 7);
        }

        // Windows 11 running/focused pill indicator under app icons 1..4
        if (i >= 1 && i <= 4 && g_ui.wins[i - 1].open) {
            const bool is_focused = (!g_ui.wins[i - 1].minimized && top_win == (i - 1));
            const int pw = is_focused ? 16 : 6;
            fill_rect(bx + (40 - pw) / 2, by + 32, pw, 3, is_focused ? 0x00F5D4 : 0x94A3B8);
        }
    }

    // Right System Tray (ETH0, SATA, RAM, CMOS RTC Clock)
    constexpr int tray_w = 318;
    const int tray_x     = g_ui.screen_w - tray_w - 14;
    fill_round_rect(tray_x, g_ui.taskbar_y + 8, tray_w, 32, 6, 0x101E34);
    stroke_round_rect(tray_x, g_ui.taskbar_y + 8, tray_w, 32, 6, 0x223B5C);

    // ETH0 status dot
    fill_round_rect(tray_x + 10, g_ui.taskbar_y + 20, 7, 7, 3, (g_ui.sys_tel.net_link_up != 0U) ? 0x4ADE80 : 0xF87171);

    const uint32_t used_mib = g_ui.sys_tel.ram_used_kib / 1024U;
    const uint32_t hh       = static_cast<uint32_t>(g_ui.sys_tel.rtc_hour);
    const uint32_t mm       = static_cast<uint32_t>(g_ui.sys_tel.rtc_min);
    const uint32_t ss       = static_cast<uint32_t>(g_ui.sys_tel.rtc_sec);

    size_t p = 0;
    p = fmt_str(g_ui.fmt, p, "ETH0 | SATA | ");
    p = fmt_u32(g_ui.fmt, p, used_mib);
    p = fmt_str(g_ui.fmt, p, "M | RTC ");
    p = fmt_u32_pad2(g_ui.fmt, p, hh);
    p = fmt_str(g_ui.fmt, p, ":");
    p = fmt_u32_pad2(g_ui.fmt, p, mm);
    p = fmt_str(g_ui.fmt, p, ":");
    p = fmt_u32_pad2(g_ui.fmt, p, ss);
    draw_text(g_ui.fmt, tray_x + 22, g_ui.taskbar_y + 20, 0xE2E8F0);
}

// --- Unified Dirty-Rectangle Compositor & Zero-Copy DMA Flip ---

void compose_region_to_backbuf(int rx, int ry, int rw, int rh) noexcept {
    const int x0 = clamp_i(rx, 0, g_ui.screen_w);
    const int y0 = clamp_i(ry, 0, g_ui.screen_h);
    const int x1 = clamp_i(rx + rw, 0, g_ui.screen_w);
    const int y1 = clamp_i(ry + rh, 0, g_ui.screen_h);
    if (x0 >= x1 || y0 >= y1) {
        return;
    }

    g_ui.clip = Rect{x0, y0, x1 - x0, y1 - y0};

    render_wallpaper_clipped();

    if (y0 < g_ui.taskbar_y) {
        draw_desktop_shortcuts();

        const int top_id = g_ui.z_order[3];
        for (int z = 0; z < 4; ++z) {
            const int wid = g_ui.z_order[z];
            const FloatingWindow& w = g_ui.wins[wid];
            if (w.open && !w.minimized) {
                const Rect wbounds{w.x - 2, w.y - 2, w.w + 8, w.h + 9};
                if (rects_intersect(g_ui.clip, wbounds)) {
                    draw_floating_window(wid, wid == top_id && !g_ui.start_open);
                }
            }
        }

        if (g_ui.start_open) {
            const int sw = 440;
            const int sh = 350;
            const int sx = (g_ui.screen_w - sw) / 2;
            const int sy = g_ui.taskbar_y - sh - 8;
            const Rect sbounds{sx - 4, sy - 4, sw + 10, sh + 10};
            if (rects_intersect(g_ui.clip, sbounds)) {
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

void flip_full() noexcept {
    fipsos::sys_ioctl(static_cast<uint64_t>(g_ui.fb_fd), fipsos::FB_IOCTL_FLIP_BUFFER, 0ULL);
}

void flip_dirty_rect(const Rect& r) noexcept {
    const int x0 = clamp_i(r.x, 0, g_ui.screen_w);
    const int y0 = clamp_i(r.y, 0, g_ui.screen_h);
    const int x1 = clamp_i(r.x + r.w, 0, g_ui.screen_w);
    const int y1 = clamp_i(r.y + r.h, 0, g_ui.screen_h);
    if (x0 >= x1 || y0 >= y1) {
        return;
    }
    const uint64_t packed = fipsos::pack_dirty_rect(
        static_cast<uint16_t>(x0),
        static_cast<uint16_t>(y0),
        static_cast<uint16_t>(x1 - x0),
        static_cast<uint16_t>(y1 - y0)
    );
    fipsos::sys_ioctl(static_cast<uint64_t>(g_ui.fb_fd), fipsos::FB_IOCTL_FLIP_BUFFER, packed);
}

void repaint_and_flip(const Rect& r) noexcept {
    compose_region_to_backbuf(r.x, r.y, r.w, r.h);
    flip_dirty_rect(r);
}

// --- Window Z-Order & Input Event Dispatch ---

Rect win_shadow_rect(const FloatingWindow& w) noexcept {
    return Rect{w.x - 2, w.y - 2, w.w + 8, w.h + 9};
}

void focus_window(int wid) noexcept {
    if (wid < 0 || wid >= 4) {
        return;
    }
    g_ui.wins[wid].open      = true;
    g_ui.wins[wid].minimized = false;
    int pos = 0;
    for (int i = 0; i < 4; ++i) {
        if (g_ui.z_order[i] == wid) {
            pos = i;
            break;
        }
    }
    for (int i = pos; i < 3; ++i) {
        g_ui.z_order[i] = g_ui.z_order[i + 1];
    }
    g_ui.z_order[3] = wid;
}

void handle_left_press(int mx, int my) noexcept {
    const int dock_x0 = (g_ui.screen_w - 288) / 2;
    constexpr int dock_stride = 48;

    // 1. Taskbar clicks
    if (my >= g_ui.taskbar_y) {
        for (int i = 0; i < 6; ++i) {
            const int bx = dock_x0 + i * dock_stride;
            const int by = g_ui.taskbar_y + 6;
            if (pt_in_rect(mx, my, bx, by, 40, 36)) {
                if (i == 0) {
                    g_ui.start_open = !g_ui.start_open;
                    const int sx = (g_ui.screen_w - 440) / 2;
                    const int sy = g_ui.taskbar_y - 350 - 6;
                    repaint_and_flip(Rect{sx - 6, sy - 6, 440 + 12, 350 + 12});
                } else if (i >= 1 && i <= 4) {
                    const int wid = i - 1;
                    g_ui.start_open = false;
                    if (g_ui.wins[wid].open && !g_ui.wins[wid].minimized && g_ui.z_order[3] == wid) {
                        g_ui.wins[wid].minimized = true;
                    } else {
                        focus_window(wid);
                    }
                    compose_all_to_backbuf();
                    flip_full();
                } else if (i == 5) {
                    g_ui.should_exit = true;
                }
                return;
            }
        }
        return;
    }

    // 2. Start Menu clicks if open
    if (g_ui.start_open) {
        const int sx = (g_ui.screen_w - 440) / 2;
        const int sy = g_ui.taskbar_y - 350 - 6;
        constexpr int sw = 440;
        constexpr int sh = 350;
        if (pt_in_rect(mx, my, sx, sy, sw, sh)) {
            // Button 1: Probar Fault (#PF)
            if (pt_in_rect(mx, my, sx + 82, sy + sh - 40, 110, 28)) {
                action_launch_elf("/bin/fault_test.elf", "fault_test.elf");
                return;
            }
            // Button 2: Salir a TTY
            if (pt_in_rect(mx, my, sx + 82 + 116, sy + sh - 40, 110, 28)) {
                g_ui.should_exit = true;
                return;
            }
            // Button 3: Apagar (ACPI S5)
            if (pt_in_rect(mx, my, sx + 82 + 232, sy + sh - 40, 110, 28)) {
                fipsos::sys_reboot(0, 0, 0, 0);
                return;
            }
            for (int i = 0; i < 6; ++i) {
                const int tx = sx + 20 + (i % 3) * 140;
                const int ty = sy + 78 + (i / 3) * 96;
                if (pt_in_rect(mx, my, tx, ty, 120, 84)) {
                    g_ui.start_open = false;
                    if (i <= 3) {
                        focus_window(i);
                        compose_all_to_backbuf();
                        flip_full();
                    } else if (i == 4) {
                        action_launch_elf("/bin/c_std_app.elf", "c_std_app.elf");
                    } else {
                        g_ui.should_exit = true; // Salir a consola TTY
                    }
                    return;
                }
            }
            return;
        }
        // Clicked outside Start Menu -> close it cleanly
        g_ui.start_open = false;
        repaint_and_flip(Rect{sx - 6, sy - 6, sw + 12, sh + 12});
    }

    // 3. Floating Windows (front-to-back Z-order hit test)
    for (int z = 3; z >= 0; --z) {
        const int wid = g_ui.z_order[z];
        FloatingWindow& w = g_ui.wins[wid];
        if (!w.open || w.minimized) {
            continue;
        }
        if (!pt_in_rect(mx, my, w.x, w.y, w.w, w.h)) {
            continue;
        }

        const bool z_changed = (z != 3);
        focus_window(wid);

        // Minimize / Close buttons
        const int min_x = w.x + w.w - 58;
        const int cls_x = w.x + w.w - 30;
        const int btn_y = w.y + 6;
        if (pt_in_rect(mx, my, min_x, btn_y, 24, 20)) {
            w.minimized = true;
            compose_all_to_backbuf();
            flip_full();
            return;
        }
        if (pt_in_rect(mx, my, cls_x, btn_y, 24, 20)) {
            w.open = false;
            compose_all_to_backbuf();
            flip_full();
            return;
        }

        // Titlebar drag start
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

        // Client-area interactive buttons per window
        const int by = w.y + w.h - 36;
        if (wid == 0) {
            if (pt_in_rect(mx, my, w.x + 14, by, 145, 26)) {
                action_spawn_bg_task();
            } else if (pt_in_rect(mx, my, w.x + 170, by, 145, 26)) {
                action_kill_task();
            } else if (pt_in_rect(mx, my, w.x + 326, by, 150, 26)) {
                action_refresh_net();
                compose_all_to_backbuf();
                flip_full();
            }
        } else if (wid == 1) {
            if (pt_in_rect(mx, my, w.x + 14, by, 160, 26)) {
                g_ui.yaw_rad = wrap_pi(g_ui.yaw_rad + 0.35f);
                recompute_sphere_tile();
                repaint_and_flip(win_shadow_rect(w));
                return;
            } else if (pt_in_rect(mx, my, w.x + 185, by, 165, 26)) {
                g_ui.palette_mode = (g_ui.palette_mode + 1U) & 3U;
                recompute_sphere_tile();
                repaint_and_flip(win_shadow_rect(w));
                return;
            }
        } else if (wid == 2) {
            if (pt_in_rect(mx, my, w.x + 14, by, 150, 26)) {
                action_browser_get();
            } else if (pt_in_rect(mx, my, w.x + 174, by, 150, 26)) {
                action_browser_get();
            } else if (pt_in_rect(mx, my, w.x + 334, by, 150, 26)) {
                action_cycle_bookmark();
            }
        } else if (wid == 3) {
            const int cx = w.x + 14;
            const int cy = w.y + 40;
            // Card clicks to select app
            for (int i = 0; i < 3; ++i) {
                const int card_y = cy + 28 + i * 42;
                if (pt_in_rect(mx, my, cx, card_y, w.w - 28, 38)) {
                    g_ui.appcenter_selected = i;
                    action_appcenter_cycle();
                    compose_all_to_backbuf();
                    flip_full();
                    return;
                }
            }
            if (pt_in_rect(mx, my, w.x + 14, by, 160, 26)) {
                action_appcenter_install();
            } else if (pt_in_rect(mx, my, w.x + 184, by, 140, 26)) {
                action_appcenter_launch();
            } else if (pt_in_rect(mx, my, w.x + 334, by, 130, 26)) {
                action_appcenter_cycle();
            } else if (pt_in_rect(mx, my, w.x + 474, by, 112, 26)) {
                action_save_disk();
            }
        }

        compose_all_to_backbuf();
        flip_full();
        return;
    }

    // 4. Desktop Shortcut Icons (left column)
    if (mx >= 20 && mx < 104) {
        if (pt_in_rect(mx, my, 20, 20, 84, 76)) {
            focus_window(3);
            compose_all_to_backbuf();
            flip_full();
        } else if (pt_in_rect(mx, my, 20, 108, 84, 76)) {
            focus_window(0);
            compose_all_to_backbuf();
            flip_full();
        } else if (pt_in_rect(mx, my, 20, 196, 84, 76)) {
            focus_window(1);
            compose_all_to_backbuf();
            flip_full();
        } else if (pt_in_rect(mx, my, 20, 284, 84, 76)) {
            focus_window(2);
            compose_all_to_backbuf();
            flip_full();
        } else if (pt_in_rect(mx, my, 20, 372, 84, 76)) {
            action_launch_elf("/bin/c_std_app.elf", "c_std_app.elf");
        }
    }
}

void handle_pointer_motion(int mx, int my, uint8_t btns) noexcept {
    if ((btns & fipsos::MOUSE_BTN_LEFT) != 0U) {
        if (g_ui.dragging_win >= 0) {
            FloatingWindow& w = g_ui.wins[g_ui.dragging_win];
            w.x = clamp_i(mx - g_ui.drag_off_x, 8, g_ui.screen_w - w.w - 8);
            w.y = clamp_i(my - g_ui.drag_off_y, 8, g_ui.taskbar_y - w.h - 6);
            compose_all_to_backbuf();
            flip_full();
            return;
        }

        // Interactive 3D sphere drag inside Window 1 viewport
        const FloatingWindow& w1 = g_ui.wins[1];
        if (w1.open && !w1.minimized && g_ui.z_order[3] == 1) {
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

    // Dock hover highlight dirty update
    const int dock_x0 = (g_ui.screen_w - 288) / 2;
    int new_dock = -1;
    if (my >= g_ui.taskbar_y) {
        for (int i = 0; i < 6; ++i) {
            if (pt_in_rect(mx, my, dock_x0 + i * 48, g_ui.taskbar_y + 6, 40, 36)) {
                new_dock = i;
                break;
            }
        }
    }
    if (new_dock != g_ui.hover_dock) {
        g_ui.hover_dock = new_dock;
        repaint_and_flip(Rect{dock_x0 - 4, g_ui.taskbar_y, 290, g_ui.taskbar_h});
    }

    if (g_ui.start_open) {
        const int sx = (g_ui.screen_w - 440) / 2;
        const int sy = g_ui.taskbar_y - 350 - 6;
        int new_tile = -1;
        for (int i = 0; i < 6; ++i) {
            const int tx = sx + 20 + (i % 3) * 140;
            const int ty = sy + 78 + (i / 3) * 96;
            if (pt_in_rect(mx, my, tx, ty, 120, 84)) {
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

static int s_esc_state = 0;

void handle_key(uint8_t key) noexcept {
    if (s_esc_state == 0) {
        if (key == 0x1BU) {
            s_esc_state = 1;
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
        if (key == 'A') { // Up
            g_ui.pitch_rad = clamp_f(g_ui.pitch_rad + 0.15f, -1.3f, 1.3f);
            recompute_sphere_tile();
            repaint_and_flip(win_shadow_rect(g_ui.wins[1]));
            return;
        } else if (key == 'B') { // Down
            g_ui.pitch_rad = clamp_f(g_ui.pitch_rad - 0.15f, -1.3f, 1.3f);
            recompute_sphere_tile();
            repaint_and_flip(win_shadow_rect(g_ui.wins[1]));
            return;
        } else if (key == 'C') { // Right
            g_ui.yaw_rad += 0.15f;
            if (g_ui.yaw_rad > 3.14159f) g_ui.yaw_rad -= 6.28318f;
            recompute_sphere_tile();
            repaint_and_flip(win_shadow_rect(g_ui.wins[1]));
            return;
        } else if (key == 'D') { // Left
            g_ui.yaw_rad -= 0.15f;
            if (g_ui.yaw_rad < -3.14159f) g_ui.yaw_rad += 6.28318f;
            recompute_sphere_tile();
            repaint_and_flip(win_shadow_rect(g_ui.wins[1]));
            return;
        }
        return;
    }

    if (key == 'q' || key == 'Q') {
        g_ui.should_exit = true;
    } else if (key == ' ' || key == 0x11U) {
        g_ui.start_open = !g_ui.start_open;
        const int sx = (g_ui.screen_w - 440) / 2;
        const int sy = g_ui.taskbar_y - 350 - 6;
        repaint_and_flip(Rect{sx - 6, sy - 6, 440 + 12, 350 + 12});
    } else if (key >= '1' && key <= '4') {
        const int wid = static_cast<int>(key - '1');
        g_ui.start_open = false;
        focus_window(wid);
        compose_all_to_backbuf();
        flip_full();
    } else if (key == 'c' || key == 'C') {
        action_launch_elf("/bin/c_std_app.elf", "c_std_app.elf");
        compose_all_to_backbuf();
        flip_full();
    } else if (key == 'p' || key == 'P') {
        action_ping_gateway(true);
        compose_all_to_backbuf();
        flip_full();
    } else if (key == 'g' || key == 'G') {
        action_http_wget();
        compose_all_to_backbuf();
        flip_full();
    } else if (key == 'n' || key == 'N') {
        action_run_npu();
        compose_all_to_backbuf();
        flip_full();
    } else if (key == 'r' || key == 'R') {
        action_launch_elf("/bin/npu_infer.elf", "npu_infer.elf");
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
    } else if (key == 'w' || key == 'W') {
        action_refresh_net();
        compose_all_to_backbuf();
        flip_full();
    } else if (key == 'a' || key == 'A') {
        action_appcenter_cycle();
        compose_all_to_backbuf();
        flip_full();
    } else if (key == 'l' || key == 'L') {
        action_appcenter_launch();
        compose_all_to_backbuf();
        flip_full();
    } else if (key == 'i' || key == 'I') {
        action_appcenter_install();
        compose_all_to_backbuf();
        flip_full();
    } else if (key == 'm' || key == 'M') {
        action_cycle_bookmark();
        compose_all_to_backbuf();
        flip_full();
    } else if (key == 'd' || key == 'D') {
        action_delete_sata_file();
        compose_all_to_backbuf();
        flip_full();
    } else if (key == 't' || key == 'T') {
        g_ui.yaw_rad      = wrap_pi(g_ui.yaw_rad + 0.35f);
        g_ui.palette_mode = (g_ui.palette_mode + 1U) & 3U;
        recompute_sphere_tile();
        repaint_and_flip(win_shadow_rect(g_ui.wins[1]));
    }
}

} // namespace

extern "C" void desktop_main() noexcept {
    using namespace fipsos;

    print_literal(
        "[desktop_ui.elf] FipSOs Shell UI v3.1 (4 Ventanas Flotantes | SMP Multicore | PCIe e1000e | SATA AHCI) activo!\n"
    );

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

    if (g_ui.fb_fd < 0) {
        sys_exit(1);
    }

    // Query physical display resolution & pitch reported by Limine Framebuffer
    fb_info finfo{};
    int64_t vres = sys_ioctl(static_cast<uint64_t>(g_ui.fb_fd), FB_IOCTL_GET_INFO, reinterpret_cast<uint64_t>(&finfo));
    if (vres >= 0 && finfo.width > 0 && finfo.height > 0) {
        g_ui.screen_w = static_cast<int>(finfo.width);
        g_ui.screen_h = static_cast<int>(finfo.height);
        g_ui.fb_pitch = static_cast<int>(finfo.pitch > 0 ? finfo.pitch : finfo.width * 4);
        g_ui.bpp      = static_cast<int>(finfo.bpp > 0 ? finfo.bpp : 32);
    } else {
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
            // Fallback default
            g_ui.screen_w = 1024;
            g_ui.screen_h = 768;
            g_ui.fb_pitch = 4096;
            g_ui.bpp      = 32;
        }
    }
    g_ui.taskbar_h = 48;
    g_ui.taskbar_y = g_ui.screen_h - g_ui.taskbar_h;

    const size_t kBackbufBytes = static_cast<size_t>(g_ui.screen_h) * static_cast<size_t>(g_ui.fb_pitch);
    const int64_t mapped = sys_mmap(0, kBackbufBytes, PROT_READ | PROT_WRITE, MAP_SHARED, static_cast<uint64_t>(g_ui.fb_fd), 0);
    if (mapped <= 0) {
        sys_exit(2);
    }
    g_ui.backbuf = reinterpret_cast<uint32_t*>(mapped);
    g_ui.clip    = Rect{0, 0, g_ui.screen_w, g_ui.screen_h};

    // Initialize 3D Phong Sphere state & Aurora LUTs
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
    set_cstr(g_ui.last_action, sizeof(g_ui.last_action), "Listo - Haz clic en apps o botones");

    // Configure 4 Floating Windows proportionally to detected screen resolution
    const int margin_x = 24;
    const int margin_y = 24;
    const int half_w = (g_ui.screen_w - (margin_x * 2 + 16)) / 2;
    const int half_h = (g_ui.taskbar_y - (margin_y * 2 + 16)) / 2;

    g_ui.wins[0] = FloatingWindow{0, margin_x, margin_y, half_w, half_h, true, false, 0x00F5D4, "Admin de Tareas & Rendimiento SMP"};
    g_ui.wins[1] = FloatingWindow{1, margin_x + half_w + 16, margin_y, half_w, half_h, true, false, 0x818CF8, "Visor 3D Phong — FP32 SSE2"};
    g_ui.wins[2] = FloatingWindow{2, margin_x, margin_y + half_h + 16, half_w, half_h, true, false, 0x38BDF8, "Navegador Web & HTTP Downloader"};
    g_ui.wins[3] = FloatingWindow{3, margin_x + half_w + 16, margin_y + half_h + 16, half_w, half_h, true, false, 0xFBBF24, "Centro de Aplicaciones & Consola de Ejecucion"};
    g_ui.z_order[0] = 0;
    g_ui.z_order[1] = 1;
    g_ui.z_order[2] = 2;
    g_ui.z_order[3] = 3; // Window 3 initially focused on top

    g_ui.browser_bookmark = 1;
    set_cstr(g_ui.browser_url, sizeof(g_ui.browser_url), "http://10.0.2.2:8080/host_test.txt");
    set_cstr(g_ui.browser_status, sizeof(g_ui.browser_status), "Listo para GET TCP");
    g_ui.browser_downloaded_body[0] = '\0';
    g_ui.selected_disk_file = 0;

    g_ui.appcenter_selected = 0;
    g_ui.appcenter_installed[0] = false;
    g_ui.appcenter_installed[1] = false;
    g_ui.appcenter_installed[2] = false;
    set_cstr(g_ui.appcenter_console, sizeof(g_ui.appcenter_console), "Listo. Servidor http://10.0.2.2:8080/ [TCP]");
    set_cstr(g_ui.appcenter_last_exec, sizeof(g_ui.appcenter_last_exec), "Ninguna");

    init_wallpaper_luts();
    recompute_sphere_tile();

    // Seed live initial hardware telemetry (e1000e ARP+ICMP + C11 NPU FP32 + RAM stats)
    if (g_ui.mouse_fd >= 0) {
        const int64_t mraw = sys_ioctl(static_cast<uint64_t>(g_ui.mouse_fd), MOUSE_IOCTL_POLL_STATE, 0ULL);
        if (mraw >= 0) {
            const MousePacketState ms = unpack_mouse_state(static_cast<uint64_t>(mraw));
            g_ui.mouse_x   = ms.x;
            g_ui.mouse_y   = ms.y;
            g_ui.mouse_btns = ms.buttons;
            g_ui.prev_btns  = ms.buttons;
            g_ui.mouse_seq  = ms.event_seq;
        }
    }
    action_ping_gateway(false);
    action_run_npu();
    set_cstr(g_ui.last_action, sizeof(g_ui.last_action), "Escritorio activo (ESC/TTY para consola)");

    // Activate kernel Desktop Compositor mode and perform initial Zero-Copy full-frame DMA flip
    sys_ioctl(static_cast<uint64_t>(g_ui.fb_fd), FB_IOCTL_SET_DESKTOP_MODE, 1ULL);
    compose_all_to_backbuf();
    flip_full();

    uint64_t last_tel_tick = g_ui.sys_tel.irq0_ticks;

    // Main Event-Driven Compositor Loop (cooperative yield when idle)
    while (!g_ui.should_exit) {
        bool had_event = false;

        if (g_ui.mouse_fd >= 0) {
            const int64_t mraw = sys_ioctl(static_cast<uint64_t>(g_ui.mouse_fd), MOUSE_IOCTL_POLL_STATE, 0ULL);
            if (mraw >= 0) {
                const MousePacketState ms = unpack_mouse_state(static_cast<uint64_t>(mraw));
                if (ms.event_seq != g_ui.mouse_seq || ms.buttons != g_ui.mouse_btns) {
                    had_event = true;
                    const bool left_down = (ms.buttons & MOUSE_BTN_LEFT) != 0U;
                    const bool was_down  = (g_ui.prev_btns & MOUSE_BTN_LEFT) != 0U;

                    g_ui.mouse_x    = ms.x;
                    g_ui.mouse_y    = ms.y;
                    g_ui.mouse_btns = ms.buttons;
                    g_ui.mouse_seq  = ms.event_seq;

                    if (left_down && !was_down) {
                        handle_left_press(ms.x, ms.y);
                    } else if (!left_down && was_down) {
                        g_ui.dragging_win = -1;
                    } else {
                        handle_pointer_motion(ms.x, ms.y, ms.buttons);
                    }
                    g_ui.prev_btns = ms.buttons;
                }
            }
        }

        const int64_t key = sys_ioctl(static_cast<uint64_t>(g_ui.fb_fd), FB_IOCTL_POLL_KEY, 0ULL);
        if (key > 0) {
            had_event = true;
            handle_key(static_cast<uint8_t>(key & 0xFF));
        }

        if (!had_event) {
            for (int spin = 0; spin < 4096; ++spin) {
                asm volatile("pause");
            }
            sys_sched_yield();
            refresh_kernel_telemetry();
            if (g_ui.sys_tel.irq0_ticks >= last_tel_tick + 50ULL) {
                last_tel_tick = g_ui.sys_tel.irq0_ticks;
                push_spark_sample(static_cast<uint8_t>(10U + ((g_ui.sys_tel.daemon_switches * 5U) % 24U)));
                // Surgical dirty-rect update for System Tray clock & Monitor window if visible
                repaint_and_flip(Rect{g_ui.screen_w - 334, g_ui.taskbar_y + 6, 322, 36});
                if (g_ui.wins[0].open && !g_ui.wins[0].minimized && g_ui.dragging_win < 0) {
                    repaint_and_flip(win_shadow_rect(g_ui.wins[0]));
                }
            }
        }
    }

    // Restore clean TTY console surface before returning to parent Shell
    sys_ioctl(static_cast<uint64_t>(g_ui.fb_fd), FB_IOCTL_SET_DESKTOP_MODE, 0ULL);

    if (g_ui.model_fd >= 0) {
        sys_close(static_cast<uint64_t>(g_ui.model_fd));
    }
    if (g_ui.accel_fd >= 0) {
        sys_close(static_cast<uint64_t>(g_ui.accel_fd));
    }
    if (g_ui.net_fd >= 0) {
        sys_close(static_cast<uint64_t>(g_ui.net_fd));
    }
    if (g_ui.mouse_fd >= 0) {
        sys_close(static_cast<uint64_t>(g_ui.mouse_fd));
    }
    sys_close(static_cast<uint64_t>(g_ui.fb_fd));

    print_literal("[desktop_ui.elf] Sesion grafica finalizada limpiamente. Retornando a TTY fipsos@v3.1:/$\n");
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
