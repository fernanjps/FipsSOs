#include "ui_renderer.hpp"
#include "simd_gfx3d.hpp"

namespace fipsos::ui {

namespace {

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

inline int corner_inset(int dy_from_edge, int r) noexcept {
    if (r <= 0 || dy_from_edge >= r) {
        return 0;
    }
    const int d = r - 1 - dy_from_edge;
    int inset = 0;
    while ((r - inset - 1) * (r - inset - 1) + d * d > r * r && inset < r) {
        inset += 1;
    }
    return inset;
}

} // namespace

size_t str_len(const char* s) noexcept {
    size_t n = 0;
    while (s[n] != '\0') {
        n += 1;
    }
    return n;
}

bool str_eq(const char* a, const char* b) noexcept {
    while (*a != '\0' && *b != '\0') {
        if (*a != *b) return false;
        ++a;
        ++b;
    }
    return *a == *b;
}

void set_cstr(char* dst, size_t cap, const char* src) noexcept {
    if (cap == 0) return;
    size_t i = 0;
    while (i + 1 < cap && src[i] != '\0') {
        dst[i] = src[i];
        i += 1;
    }
    dst[i] = '\0';
}

size_t fmt_str(char* buf, size_t pos, const char* s) noexcept {
    return fmt_str_cap(buf, 128, pos, s);
}

size_t fmt_str_cap(char* buf, size_t cap, size_t pos, const char* s) noexcept {
    if (cap == 0) return pos;
    while (*s != '\0' && pos + 1 < cap) {
        buf[pos++] = *s++;
    }
    if (pos < cap) buf[pos] = '\0';
    return pos;
}

size_t fmt_u32(char* buf, size_t pos, uint32_t val, size_t cap) noexcept {
    if (cap == 0) return pos;
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
    while (n > 0 && pos + 1 < cap) {
        buf[pos++] = tmp[--n];
    }
    if (pos < cap) buf[pos] = '\0';
    return pos;
}

size_t fmt_u32_pad2(char* buf, size_t pos, uint32_t val, size_t cap) noexcept {
    if (cap < 2) {
        return pos;
    }
    if (val < 10U && pos + 1 < cap) {
        buf[pos++] = '0';
    }
    return fmt_u32(buf, pos, val, cap);
}

size_t fmt_hex_byte(char* buf, size_t pos, uint8_t b, size_t cap) noexcept {
    constexpr char kHex[] = "0123456789ABCDEF";
    if (pos + 2 < cap) {
        buf[pos++] = kHex[(b >> 4) & 0xF];
        buf[pos++] = kHex[b & 0xF];
        buf[pos]   = '\0';
    }
    return pos;
}

size_t fmt_hex_u64(char* buf, size_t pos, uint64_t val, size_t cap) noexcept {
    if (cap == 0) return pos;
    constexpr char kHex[] = "0123456789abcdef";
    pos = fmt_str_cap(buf, cap, pos, "0x");
    if (val == 0ULL) {
        if (pos + 1 < cap) {
            buf[pos++] = '0';
        }
        if (pos < cap) buf[pos] = '\0';
        return pos;
    }
    char tmp[16];
    size_t n = 0;
    while (val > 0ULL && n < sizeof(tmp)) {
        tmp[n++] = kHex[val & 0xFULL];
        val >>= 4;
    }
    while (n > 0 && pos + 1 < cap) {
        buf[pos++] = tmp[--n];
    }
    if (pos < cap) buf[pos] = '\0';
    return pos;
}

uint32_t blend_rgb(uint32_t dst, uint32_t src, uint32_t alpha) noexcept {
    const uint32_t inv = 256U - alpha;
    const uint32_t r   = (((src >> 16) & 0xFFU) * alpha + ((dst >> 16) & 0xFFU) * inv) >> 8;
    const uint32_t g   = (((src >> 8) & 0xFFU) * alpha + ((dst >> 8) & 0xFFU) * inv) >> 8;
    const uint32_t b   = ((src & 0xFFU) * alpha + (dst & 0xFFU) * inv) >> 8;
    return (r << 16) | (g << 8) | b;
}

void put_px(int x, int y, uint32_t rgb) noexcept {
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
    if (x0 >= x1 || y0 >= y1) return;
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
    if (x0 >= x1 || y0 >= y1) return;
    const int pitch_pixels = g_ui.fb_pitch / 4;
    for (int py = y0; py < y1; ++py) {
        uint32_t* row = g_ui.backbuf + py * pitch_pixels;
        for (int px = x0; px < x1; ++px) {
            row[px] = blend_rgb(row[px], rgb, alpha_256);
        }
    }
}

void fill_round_rect(int x, int y, int w, int h, int r, uint32_t rgb) noexcept {
    if (w <= 0 || h <= 0) return;
    const int y0 = max_i(y, g_ui.clip.y);
    const int y1 = min_i(y + h, g_ui.clip.y + g_ui.clip.h);
    const int pitch_pixels = g_ui.fb_pitch / 4;
    for (int py = y0; py < y1; ++py) {
        const int edge_d = min_i(py - y, (y + h - 1) - py);
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
    if (w <= 0 || h <= 0) return;
    const int y0 = max_i(y, g_ui.clip.y);
    const int y1 = min_i(y + h, g_ui.clip.y + g_ui.clip.h);
    const int pitch_pixels = g_ui.fb_pitch / 4;
    for (int py = y0; py < y1; ++py) {
        const int edge_d = min_i(py - y, (y + h - 1) - py);
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
    if (w <= 2 || h <= 2) return;
    for (int py = y; py < y + h; ++py) {
        const int edge_d = min_i(py - y, (y + h - 1) - py);
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

void draw_char(char ch, int x, int y, uint32_t fg, int max_x) noexcept {
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
        if (bits == 0) continue;
        for (int gx = 0; gx < 8; ++gx) {
            if ((bits & (0x80U >> gx)) != 0U) {
                put_px(x + gx, y + gy, fg);
            }
        }
    }
}

void draw_text(const char* s, int x, int y, uint32_t fg, int max_x) noexcept {
    const int limit_x = min_i(max_x, g_ui.screen_w - 8);
    int cx = x;
    while (*s != '\0') {
        if (cx > limit_x) break;
        draw_char(*s, cx, y, fg, limit_x);
        cx += 8;
        ++s;
    }
}

void draw_text_centered(const char* s, int rx, int ry, int rw, int rh, uint32_t fg) noexcept {
    const int tw = static_cast<int>(str_len(s)) * 8;
    const int tx = max_i(rx + 4, rx + (rw - tw) / 2);
    const int ty = ry + (rh - 8) / 2;
    draw_text(s, tx, ty, fg, rx + rw - 4);
}

void draw_button(int x, int y, int w, int h, const char* label, uint32_t bg, uint32_t border, uint32_t fg) noexcept {
    const bool hovered  = pt_in_rect(g_ui.mouse_x, g_ui.mouse_y, x, y, w, h);
    const uint32_t fill = hovered ? blend_rgb(bg, 0xFFFFFF, 38) : bg;
    const uint32_t brd  = hovered ? 0x00F5D4 : border;
    fill_round_rect(x, y, w, h, 5, fill);
    stroke_round_rect(x, y, w, h, 5, brd);
    draw_text_centered(label, x, y, w, h, fg);
}

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

void draw_mouse_cursor_sprite(int mx, int my, uint8_t btns) noexcept {
    constexpr uint8_t kArrow[10][7] = {
        {1,0,0,0,0,0,0},
        {1,1,0,0,0,0,0},
        {1,2,1,0,0,0,0},
        {1,2,2,1,0,0,0},
        {1,2,2,2,1,0,0},
        {1,2,2,2,2,1,0},
        {1,2,2,1,1,1,1},
        {1,2,1,0,0,0,0},
        {1,1,0,0,0,0,0},
        {1,0,0,0,0,0,0}
    };
    const uint32_t core = ((btns & fipsos::MOUSE_BTN_LEFT) != 0U) ? 0xFFD166 : 0x00F5D4;
    for (int dy = 0; dy < 10; ++dy) {
        for (int dx = 0; dx < 7; ++dx) {
            if (kArrow[dy][dx] == 1U) put_px(mx + dx, my + dy, 0x0D1526);
            else if (kArrow[dy][dx] == 2U) put_px(mx + dx, my + dy, core);
        }
    }
}

void init_wallpaper_luts() noexcept {
    const int w = (g_ui.screen_w > 0 && g_ui.screen_w <= CompositorState::kMaxW) ? g_ui.screen_w : 1024;
    const int h = (g_ui.screen_h > 0 && g_ui.screen_h <= CompositorState::kMaxH) ? g_ui.screen_h : 768;
    const bool slate = (g_ui.theme_id == 1U);
    for (int x = 0; x < w; ++x) {
        const float fx    = static_cast<float>(x) * (1.0f / static_cast<float>(w));
        const float bloom = sinf_fast((fx - 0.15f) * kPi);
        const float bpos  = (bloom > 0.0f) ? bloom : 0.0f;
        if (slate) {
            g_ui.bg_xr[x] = static_cast<uint8_t>(12.0f + bpos * 18.0f);
            g_ui.bg_xg[x] = static_cast<uint8_t>(14.0f + bpos * 22.0f);
            g_ui.bg_xb[x] = static_cast<uint8_t>(18.0f + bpos * 28.0f);
        } else {
            g_ui.bg_xr[x] = static_cast<uint8_t>(6.0f + bpos * 18.0f);
            g_ui.bg_xg[x] = static_cast<uint8_t>(14.0f + bpos * 44.0f);
            g_ui.bg_xb[x] = static_cast<uint8_t>(30.0f + bpos * 72.0f);
        }
    }
    for (int y = 0; y < h; ++y) {
        const float fy   = static_cast<float>(y) * (1.0f / static_cast<float>(h));
        const float wave = sinf_fast((1.0f - fy) * 2.2f);
        const float wpos = (wave > 0.0f) ? wave : 0.0f;
        if (slate) {
            g_ui.bg_yr[y] = static_cast<uint8_t>(10.0f + fy * 10.0f + wpos * 10.0f);
            g_ui.bg_yg[y] = static_cast<uint8_t>(12.0f + (1.0f - fy) * 14.0f + wpos * 12.0f);
            g_ui.bg_yb[y] = static_cast<uint8_t>(16.0f + (1.0f - fy) * 18.0f + wpos * 16.0f);
        } else {
            g_ui.bg_yr[y] = static_cast<uint8_t>(4.0f + fy * 10.0f + wpos * 8.0f);
            g_ui.bg_yg[y] = static_cast<uint8_t>(10.0f + (1.0f - fy) * 28.0f + wpos * 22.0f);
            g_ui.bg_yb[y] = static_cast<uint8_t>(22.0f + (1.0f - fy) * 48.0f + wpos * 34.0f);
        }
    }
}

void render_wallpaper_clipped() noexcept {
    const int x0 = max_i(0, g_ui.clip.x);
    const int y0 = max_i(0, g_ui.clip.y);
    const int x1 = min_i(g_ui.screen_w, g_ui.clip.x + g_ui.clip.w);
    const int y1 = min_i(g_ui.screen_h, g_ui.clip.y + g_ui.clip.h);
    if (x0 >= x1 || y0 >= y1) return;

    const int pitch_pixels = g_ui.fb_pitch / 4;
    for (int y = y0; y < y1; ++y) {
        uint32_t*      row = g_ui.backbuf + y * pitch_pixels;
        const uint32_t yr  = g_ui.bg_yr[y];
        const uint32_t yg  = g_ui.bg_yg[y];
        const uint32_t yb  = g_ui.bg_yb[y];
        for (int x = x0; x < x1; ++x) {
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
    fipsos::gfx3d::render_scene_zbuf_sse2(
        g_ui.sphere_tile,
        kSphereSize,
        g_ui.yaw_rad,
        g_ui.pitch_rad,
        g_ui.light_dir,
        g_ui.palette_mode,
        &g_ui.simd_trace
    );
}

void flip_full() noexcept {
    fipsos::sys_ioctl(static_cast<uint64_t>(g_ui.fb_fd), fipsos::FB_IOCTL_FLIP_BUFFER, 0ULL);
}

void flip_dirty_rect(const Rect& r) noexcept {
    const int x0 = clamp_i(r.x, 0, g_ui.screen_w);
    const int y0 = clamp_i(r.y, 0, g_ui.screen_h);
    const int x1 = clamp_i(r.x + r.w, 0, g_ui.screen_w);
    const int y1 = clamp_i(r.y + r.h, 0, g_ui.screen_h);
    if (x0 >= x1 || y0 >= y1) return;
    const uint64_t packed = fipsos::pack_dirty_rect(
        static_cast<uint16_t>(x0),
        static_cast<uint16_t>(y0),
        static_cast<uint16_t>(x1 - x0),
        static_cast<uint16_t>(y1 - y0)
    );
    fipsos::sys_ioctl(static_cast<uint64_t>(g_ui.fb_fd), fipsos::FB_IOCTL_FLIP_BUFFER, packed);
}

} // namespace fipsos::ui
