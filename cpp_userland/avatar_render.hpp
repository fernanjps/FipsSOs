#pragma once

#include "fipsos_abi.hpp"

namespace {

constexpr float kPi     = 3.14159265f;
constexpr float kTwoPi  = 6.28318531f;
constexpr float kHalfPi = 1.57079633f;

/// Hardware SSE2 scalar single-precision square root (`sqrtss` on `XMM` registers).
inline float sqrtf_sse2(float x) noexcept {
    if (x <= 0.0f) {
        return 0.0f;
    }
    float out = 0.0f;
    asm volatile("sqrtss %1, %0" : "=x"(out) : "x"(x));
    return out;
}

template <typename T>
inline T wrap_pi(T rad) noexcept {
    const T pi     = static_cast<T>(kPi);
    const T two_pi = static_cast<T>(kTwoPi);
    while (rad > pi) {
        rad -= two_pi;
    }
    while (rad < -pi) {
        rad += two_pi;
    }
    return rad;
}

/// Polymorphic inline IEEE-754 floating-point sine approximation (`< 0.001f` max error on `[-pi, pi]`).
template <typename T>
inline T sinf(T rad) noexcept {
    const T x     = wrap_pi(rad);
    const T b     = static_cast<T>(4.0f / kPi);
    const T c     = static_cast<T>(-4.0f / (kPi * kPi));
    const T abs_x = (x >= static_cast<T>(0.0f)) ? x : -x;
    const T y     = b * x + c * x * abs_x;
    const T abs_y = (y >= static_cast<T>(0.0f)) ? y : -y;
    const T p     = static_cast<T>(0.225f);
    return p * (y * abs_y - y) + y;
}

/// Polymorphic inline IEEE-754 floating-point cosine approximation (`sin(rad + pi/2)`).
template <typename T>
inline T cosf(T rad) noexcept {
    return sinf(rad + static_cast<T>(kHalfPi));
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

/// Dynamic 3D Phong Avatar camera, lighting, PS/2 pointer, and FP32 NPU tensor state on Ring 3 Heap (`sys_brk`).
struct alignas(16) AvatarHeapState {
    float    yaw_rad;
    float    pitch_rad;
    float    radius_px;
    float    light_dir[3];
    alignas(16) float fp32_weights[16];
    float    normal_norm_sample;
    float    phong_peak_sample;
    uint32_t npu_energy_milli;
    uint32_t palette_mode;
    uint32_t frames_rendered;
    uint32_t npu_inferences;
    uint16_t mouse_x;
    uint16_t mouse_y;
    uint8_t  mouse_buttons;
    uint32_t mouse_seq;
    uint32_t mouse_events_handled;
    uint16_t net_tx_pkts;
    uint16_t net_rx_pkts;
    uint16_t net_rtt_us;
    uint32_t net_pings;
    int64_t  pid;
    char     fmt_buf[192];
};

/// Steers the normalized FP32 directional light `light_dir[3]` (and 3D sphere rotation on Left Click)
/// directly from the `/dev/input/mouse0` pointer coordinates `(0..1280, 0..800)`.
inline void steer_scene_from_mouse(AvatarHeapState* state, const fipsos::MousePacketState& ms) noexcept {
    state->mouse_x       = ms.x;
    state->mouse_y       = ms.y;
    state->mouse_buttons = ms.buttons;
    state->mouse_seq     = ms.event_seq;

    const float nx_ptr = (static_cast<float>(ms.x) - 640.0f) * (1.0f / 640.0f);
    const float ny_ptr = (static_cast<float>(ms.y) - 400.0f) * (1.0f / 400.0f);

    const float raw_lx  = nx_ptr * 0.90f;
    const float raw_ly  = ny_ptr * 0.90f;
    const float raw_lz  = 0.72f;
    const float inv_len = 1.0f / sqrtf_sse2(raw_lx * raw_lx + raw_ly * raw_ly + raw_lz * raw_lz);
    state->light_dir[0] = raw_lx * inv_len;
    state->light_dir[1] = raw_ly * inv_len;
    state->light_dir[2] = raw_lz * inv_len;

    if ((ms.buttons & fipsos::MOUSE_BTN_LEFT) != 0) {
        state->yaw_rad   = wrap_pi(nx_ptr * kHalfPi);
        state->pitch_rad = wrap_pi(ny_ptr * 1.15f);
    }
    if ((ms.buttons & fipsos::MOUSE_BTN_RIGHT) != 0) {
        state->palette_mode = (state->palette_mode + 1U) & 3U;
    }
}

class AvatarEngine3D {
public:
    static constexpr size_t kWidth  = 160;
    static constexpr size_t kHeight = 80;

    static void render_frame(volatile uint32_t* fb_vmo, AvatarHeapState* state) noexcept {
        const float cx    = static_cast<float>(kWidth) * 0.5f;
        const float cy    = static_cast<float>(kHeight) * 0.5f;
        const float r     = state->radius_px;
        const float inv_r = (r > 1.0f) ? (1.0f / r) : 1.0f;
        const float halo_limit = (r + 2.5f) * (r + 2.5f);

        const float cos_yaw   = cosf(state->yaw_rad);
        const float sin_yaw   = sinf(state->yaw_rad);
        const float cos_pitch = cosf(state->pitch_rad);
        const float sin_pitch = sinf(state->pitch_rad);

        const float lx = state->light_dir[0];
        const float ly = state->light_dir[1];
        const float lz = state->light_dir[2];

        const float w0 = clamp01(state->fp32_weights[0]);
        const float w1 = clamp01(state->fp32_weights[5]);
        const float w2 = clamp01(state->fp32_weights[10]);

        const uint32_t mode = state->palette_mode & 3U;
        float peak_intensity = 0.0f;

        for (size_t y = 0; y < kHeight; ++y) {
            for (size_t x = 0; x < kWidth; ++x) {
                if (y < 4) {
                    const size_t progress = (state->frames_rendered * 16U) % kWidth;
                    fb_vmo[y * kWidth + x] = (x < progress) ? 0x0000F5D4U : 0x00122238U;
                    continue;
                }
                if (y >= kHeight - 4) {
                    const uint32_t bar_col = (mode == 0) ? 0x007AE582U
                                           : (mode == 1) ? 0x00FFD166U
                                           : (mode == 2) ? 0x00C77DFFU
                                                         : 0x00FF6B6BU;
                    fb_vmo[y * kWidth + x] = ((x + state->npu_inferences * 8U) & 15U) < 10U
                                           ? bar_col
                                           : 0x000D182AU;
                    continue;
                }

                const float dx = static_cast<float>(x) - cx;
                const float dy = static_cast<float>(y) - cy;
                const float dist2_px = dx * dx + dy * dy;

                const float nx = dx * inv_r;
                const float ny = dy * inv_r;
                const float nr2 = nx * nx + ny * ny;

                uint32_t pixel = 0x000A1428U;

                if (nr2 <= 1.0f) {
                    const float nz = sqrtf_sse2(1.0f - nr2);

                    if (x == (kWidth / 2) && y == (kHeight / 2)) {
                        state->normal_norm_sample = sqrtf_sse2(nx * nx + ny * ny + nz * nz);
                    }

                    // 3D Trigonometric Rotation (Rx(pitch) * Ry(yaw) * N)
                    const float rx1 = nx * cos_yaw + nz * sin_yaw;
                    const float rz1 = -nx * sin_yaw + nz * cos_yaw;
                    const float ry2 = ny * cos_pitch - rz1 * sin_pitch;
                    const float rz2 = ny * sin_pitch + rz1 * cos_pitch;

                    const float lat_wave = sinf(ry2 * 9.424778f);
                    const float lon_wave = sinf((rx1 * 7.5f) + (rz2 * 3.5f));
                    const bool lat_ring = (lat_wave > -0.11f && lat_wave < 0.11f);
                    const bool lon_ring = (lon_wave > -0.09f && lon_wave < 0.09f);

                    // Real IEEE-754 FP32 Phong Lighting: Ambient + Lambertian Diffuse + Specular^16
                    const float n_dot_l = nx * lx + ny * ly + nz * lz;
                    const float diff    = (n_dot_l > 0.0f) ? n_dot_l : 0.0f;

                    const float refl_z = (n_dot_l > 0.0f) ? (2.0f * n_dot_l * nz - lz) : 0.0f;
                    const float s1     = (refl_z > 0.0f) ? refl_z : 0.0f;
                    const float s2     = s1 * s1;
                    const float s4     = s2 * s2;
                    const float s8     = s4 * s4;
                    const float spec   = s8 * s8;

                    const float ambient = 0.15f + 0.08f * w0;
                    const float phong   = clamp01(ambient + 0.68f * diff + 0.55f * spec);
                    if (phong > peak_intensity) {
                        peak_intensity = phong;
                    }

                    if (lat_ring || lon_ring) {
                        const float wire_boost = 0.55f + 0.45f * diff;
                        const uint32_t c_hi = static_cast<uint32_t>(clamp01(wire_boost + 0.35f * spec) * 255.0f);
                        const uint32_t c_lo = static_cast<uint32_t>(clamp01(wire_boost * 0.65f) * 255.0f);
                        pixel = (mode == 0) ? ((0x10U << 16) | (c_hi << 8) | c_lo)
                              : (mode == 1) ? ((c_lo << 16) | (c_hi << 8) | 0x40U)
                              : (mode == 2) ? ((c_hi << 16) | (c_lo << 8) | c_hi)
                                            : ((c_hi << 16) | (c_lo << 8) | 0x20U);
                    } else {
                        float r_ch = 0.22f * phong + 0.55f * spec + 0.12f * w1;
                        float g_ch = 0.72f * phong + 0.40f * spec + 0.10f * w0;
                        float b_ch = 0.96f * phong + 0.45f * spec + 0.08f * w2;

                        if (mode == 1) {
                            r_ch = 0.18f * phong + 0.45f * spec;
                            g_ch = 0.94f * phong + 0.40f * spec + 0.10f * w1;
                            b_ch = 0.64f * phong + 0.45f * spec;
                        } else if (mode == 2) {
                            r_ch = 0.90f * phong + 0.45f * spec + 0.10f * w0;
                            g_ch = 0.34f * phong + 0.45f * spec;
                            b_ch = 0.95f * phong + 0.45f * spec + 0.10f * w2;
                        } else if (mode == 3) {
                            r_ch = 0.96f * phong + 0.40f * spec;
                            g_ch = 0.62f * phong + 0.45f * spec + 0.10f * w0;
                            b_ch = 0.24f * phong + 0.50f * spec;
                        }

                        const uint32_t red   = static_cast<uint32_t>(clamp01(r_ch) * 255.0f);
                        const uint32_t green = static_cast<uint32_t>(clamp01(g_ch) * 255.0f);
                        const uint32_t blue  = static_cast<uint32_t>(clamp01(b_ch) * 255.0f);
                        pixel = ((red & 0xFFU) << 16) | ((green & 0xFFU) << 8) | (blue & 0xFFU);
                    }
                } else if (dist2_px <= halo_limit) {
                    const uint32_t net_traffic =
                        static_cast<uint32_t>(state->net_tx_pkts) +
                        static_cast<uint32_t>(state->net_rx_pkts);
                    const uint32_t band = (net_traffic + static_cast<uint32_t>((x + y) >> 2)) & 3U;
                    pixel = (band == 0U) ? 0x0000F5D4U
                          : (band == 1U) ? 0x003A86FFU
                          : (band == 2U) ? 0x00FFD166U
                                         : 0x00C77DFFU;
                } else if (x == (kWidth / 2) || y == (kHeight / 2)) {
                    if (((x + y) & 3U) == 0U) {
                        pixel = 0x0026547CU;
                    }
                }

                fb_vmo[y * kWidth + x] = pixel;
            }
        }

        // Interactive 3D Viewport Pointer Cursor mapped from `/dev/input/mouse0` (1280x800 -> 160x80)
        size_t cur_x = (static_cast<size_t>(state->mouse_x) * kWidth) / 1280U;
        size_t cur_y = (static_cast<size_t>(state->mouse_y) * kHeight) / 800U;
        if (cur_x < 4U) { cur_x = 4U; }
        if (cur_x > kWidth - 5U) { cur_x = kWidth - 5U; }
        if (cur_y < 6U) { cur_y = 6U; }
        if (cur_y > kHeight - 7U) { cur_y = kHeight - 7U; }

        const uint32_t ptr_col = ((state->mouse_buttons & fipsos::MOUSE_BTN_LEFT) != 0)  ? 0x00FFD166U
                               : ((state->mouse_buttons & fipsos::MOUSE_BTN_RIGHT) != 0) ? 0x00FF6B6BU
                                                                                         : 0x00FFFFFFU;
        for (int d = -3; d <= 3; ++d) {
            const size_t px = static_cast<size_t>(static_cast<int>(cur_x) + d);
            const size_t py = static_cast<size_t>(static_cast<int>(cur_y) + d);
            fb_vmo[cur_y * kWidth + px] = ptr_col;
            fb_vmo[py * kWidth + cur_x] = ptr_col;
        }
        fb_vmo[(cur_y - 1U) * kWidth + (cur_x - 1U)] = 0x0000F5D4U;
        fb_vmo[(cur_y - 1U) * kWidth + (cur_x + 1U)] = 0x0000F5D4U;
        fb_vmo[(cur_y + 1U) * kWidth + (cur_x - 1U)] = 0x0000F5D4U;
        fb_vmo[(cur_y + 1U) * kWidth + (cur_x + 1U)] = 0x0000F5D4U;

        state->phong_peak_sample = peak_intensity;
        state->frames_rendered += 1;
    }
};

} // namespace
