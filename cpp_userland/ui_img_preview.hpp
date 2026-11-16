#pragma once
#include "ui_renderer.hpp"

namespace fipsos::ui {

struct ImgPreviewState {
    int      w;
    int      h;
    bool     active;
    uint32_t pixels[128 * 128];
};

inline ImgPreviewState g_img_prev{};

inline void clear_image_preview() noexcept {
    g_img_prev.active = false;
    g_img_prev.w = 0;
    g_img_prev.h = 0;
}

inline uint16_t read_le16(const uint8_t* p) noexcept {
    return static_cast<uint16_t>(p[0]) | (static_cast<uint16_t>(p[1]) << 8);
}

inline uint32_t read_le32(const uint8_t* p) noexcept {
    return static_cast<uint32_t>(p[0]) | (static_cast<uint32_t>(p[1]) << 8)
         | (static_cast<uint32_t>(p[2]) << 16) | (static_cast<uint32_t>(p[3]) << 24);
}

inline bool decode_bmp_preview(const uint8_t* data, size_t len) noexcept {
    if (len < 54 || data[0] != 'B' || data[1] != 'M') return false;
    const uint32_t off = read_le32(data + 10);
    const int32_t  w   = static_cast<int32_t>(read_le32(data + 18));
    const int32_t  sh  = static_cast<int32_t>(read_le32(data + 22));
    const uint16_t bpp = read_le16(data + 28);
    const uint32_t comp = read_le32(data + 30);
    const bool top_down = (sh < 0);
    const int32_t h = top_down ? -sh : sh;
    if (w <= 0 || h <= 0 || comp != 0 || (bpp != 24 && bpp != 32) || off >= len) return false;

    const int bytes_pp = bpp / 8;
    const size_t stride = ((static_cast<size_t>(w) * bytes_pp + 3U) / 4U) * 4U;
    const int dw = (w > 128) ? 128 : w;
    const int dh = (h > 128) ? 128 : h;
    for (int y = 0; y < dh; ++y) {
        const int sy = (y * h) / dh;
        const int row = top_down ? sy : (h - 1 - sy);
        const size_t row_off = off + static_cast<size_t>(row) * stride;
        if (row_off + static_cast<size_t>(w) * bytes_pp > len) break;
        for (int x = 0; x < dw; ++x) {
            const int sx = (x * w) / dw;
            const uint8_t* px = data + row_off + static_cast<size_t>(sx) * bytes_pp;
            g_img_prev.pixels[y * dw + x] =
                (static_cast<uint32_t>(px[2]) << 16) | (static_cast<uint32_t>(px[1]) << 8) | px[0];
        }
    }
    g_img_prev.w = dw;
    g_img_prev.h = dh;
    g_img_prev.active = true;
    return true;
}

inline int ppm_next_int(const uint8_t* data, size_t len, size_t& pos) noexcept {
    while (pos < len) {
        if (data[pos] == '#') { while (pos < len && data[pos] != '\n') ++pos; }
        else if (data[pos] <= ' ') { ++pos; }
        else break;
    }
    int v = 0;
    while (pos < len && data[pos] >= '0' && data[pos] <= '9') {
        v = v * 10 + (data[pos++] - '0');
    }
    return v;
}

inline bool decode_ppm_preview(const uint8_t* data, size_t len) noexcept {
    if (len < 11 || data[0] != 'P' || data[1] != '6') return false;
    size_t pos = 2;
    const int w = ppm_next_int(data, len, pos);
    const int h = ppm_next_int(data, len, pos);
    const int mv = ppm_next_int(data, len, pos);
    if (pos < len && data[pos] <= ' ') ++pos;
    if (w <= 0 || h <= 0 || mv <= 0 || pos >= len) return false;
    const int dw = (w > 128) ? 128 : w;
    const int dh = (h > 128) ? 128 : h;
    for (int y = 0; y < dh; ++y) {
        const int sy = (y * h) / dh;
        for (int x = 0; x < dw; ++x) {
            const int sx = (x * w) / dw;
            const size_t idx = pos + static_cast<size_t>(sy * w + sx) * 3U;
            if (idx + 2 < len) {
                g_img_prev.pixels[y * dw + x] =
                    (static_cast<uint32_t>(data[idx]) << 16)
                    | (static_cast<uint32_t>(data[idx + 1]) << 8) | data[idx + 2];
            }
        }
    }
    g_img_prev.w = dw;
    g_img_prev.h = dh;
    g_img_prev.active = true;
    return true;
}

inline bool try_load_image_preview(const char* full_path, const char* name) noexcept {
    const size_t n = str_len(name);
    const bool is_bmp = (n >= 4) && str_eq(name + n - 4, ".bmp");
    const bool is_ppm = (n >= 4) && str_eq(name + n - 4, ".ppm");
    if (!is_bmp && !is_ppm) return false;

    const int64_t fd = fipsos::sys_openat(fipsos::AT_FDCWD, full_path, fipsos::O_RDONLY, 0);
    if (fd < 0) return false;
    static uint8_t img_raw[65536];
    size_t got = 0;
    while (got < sizeof(img_raw)) {
        const int64_t r = fipsos::sys_read(static_cast<uint64_t>(fd), img_raw + got, sizeof(img_raw) - got);
        if (r <= 0) break;
        got += static_cast<size_t>(r);
    }
    fipsos::sys_close(static_cast<uint64_t>(fd));
    return is_bmp ? decode_bmp_preview(img_raw, got) : decode_ppm_preview(img_raw, got);
}

inline void draw_image_preview(int px, int py, int pw, int ph) noexcept {
    if (!g_img_prev.active || g_img_prev.w <= 0 || g_img_prev.h <= 0) return;
    int dw = pw - 8;
    int dh = (dw * g_img_prev.h) / g_img_prev.w;
    if (dh > ph - 8) { dh = ph - 8; dw = (dh * g_img_prev.w) / g_img_prev.h; }
    if (dw < 16 || dh < 16) return;
    const int ox = px + (pw - dw) / 2;
    const int oy = py + (ph - dh) / 2;
    stroke_round_rect(ox - 2, oy - 2, dw + 4, dh + 4, 3, 0x38BDF8);
    for (int y = 0; y < dh; ++y) {
        const int sy = (y * g_img_prev.h) / dh;
        for (int x = 0; x < dw; ++x) {
            const int sx = (x * g_img_prev.w) / dw;
            put_px(ox + x, oy + y, g_img_prev.pixels[sy * g_img_prev.w + sx]);
        }
    }
}

} // namespace fipsos::ui
