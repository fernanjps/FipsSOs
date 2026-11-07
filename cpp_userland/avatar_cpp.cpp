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

/// Dynamic 3D Phong Avatar camera, lighting, and FP32 NPU tensor state allocated on Ring 3 Heap (`sys_brk`).
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
    int64_t  pid;
    char     fmt_buf[192];
};

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

        // Precompute 3D trigonometric rotation matrix coefficients in IEEE-754 float (SSE2)
        const float cos_yaw   = cosf(state->yaw_rad);
        const float sin_yaw   = sinf(state->yaw_rad);
        const float cos_pitch = cosf(state->pitch_rad);
        const float sin_pitch = sinf(state->pitch_rad);

        // Normalized directional light vector L = (lx, ly, lz)
        const float lx = state->light_dir[0];
        const float ly = state->light_dir[1];
        const float lz = state->light_dir[2];

        // Neural FP32 modulation channels produced by the Ring 0 C11 GEMM driver
        const float w0 = clamp01(state->fp32_weights[0]);
        const float w1 = clamp01(state->fp32_weights[5]);
        const float w2 = clamp01(state->fp32_weights[10]);

        const uint32_t mode = state->palette_mode & 3U;
        float peak_intensity = 0.0f;

        for (size_t y = 0; y < kHeight; ++y) {
            for (size_t x = 0; x < kWidth; ++x) {
                // Top & bottom real-time HUD telemetry bars inside the 160x80 widget
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

                uint32_t pixel = 0x000A1428U; // Deep cyber background

                if (nr2 <= 1.0f) {
                    // Exact 3D unit sphere normal N = (nx, ny, nz = sqrt(1 - nx^2 - ny^2)) via SSE2 sqrtss
                    const float nz = sqrtf_sse2(1.0f - nr2);

                    if (x == (kWidth / 2) && y == (kHeight / 2)) {
                        state->normal_norm_sample = sqrtf_sse2(nx * nx + ny * ny + nz * nz);
                    }

                    // 3D Trigonometric Rotation (Rx(pitch) * Ry(yaw) * N) for latitude/longitude rings
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

                    // Reflection z-component against viewer V = (0, 0, 1): R_z = 2*(N.L)*nz - lz
                    const float refl_z = (n_dot_l > 0.0f) ? (2.0f * n_dot_l * nz - lz) : 0.0f;
                    const float s1     = (refl_z > 0.0f) ? refl_z : 0.0f;
                    const float s2     = s1 * s1;
                    const float s4     = s2 * s2;
                    const float s8     = s4 * s4;
                    const float spec   = s8 * s8; // Shininess exponent = 16

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
                    pixel = 0x00FFD166U; // Outer atmospheric glow halo
                } else if (x == (kWidth / 2) || y == (kHeight / 2)) {
                    if (((x + y) & 3U) == 0U) {
                        pixel = 0x0026547CU; // HUD reticle crosshair
                    }
                }

                fb_vmo[y * kWidth + x] = pixel;
            }
        }
        state->phong_peak_sample = peak_intensity;
        state->frames_rendered += 1;
    }
};

size_t append_str(char* dst, size_t pos, const char* lit) noexcept {
    while (*lit != '\0' && pos < 184) {
        dst[pos++] = *lit++;
    }
    return pos;
}

size_t append_u32(char* dst, size_t pos, uint32_t val) noexcept {
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
    while (n > 0 && pos < 184) {
        dst[pos++] = tmp[--n];
    }
    return pos;
}

size_t append_fp32(char* dst, size_t pos, float val) noexcept {
    if (val < 0.0f) {
        if (pos < 180) {
            dst[pos++] = '-';
        }
        val = -val;
    }
    const uint32_t scaled = static_cast<uint32_t>(val * 1000.0f + 0.5f);
    const uint32_t whole  = scaled / 1000U;
    const uint32_t frac   = scaled % 1000U;
    pos = append_u32(dst, pos, whole);
    if (pos + 4 < 186) {
        dst[pos++] = '.';
        dst[pos++] = static_cast<char>('0' + ((frac / 100U) % 10U));
        dst[pos++] = static_cast<char>('0' + ((frac / 10U) % 10U));
        dst[pos++] = static_cast<char>('0' + (frac % 10U));
    }
    return pos;
}

void sync_npu_fp32_weights(const volatile uint8_t* accel_vmo, AvatarHeapState* state) noexcept {
    if (accel_vmo == nullptr) {
        return;
    }
    const auto* dma_floats = reinterpret_cast<const volatile float*>(accel_vmo + 128);
    for (size_t i = 0; i < 16; ++i) {
        state->fp32_weights[i] = dma_floats[i];
    }
}

} // namespace

extern "C" [[noreturn]] __attribute__((section(".text._start"))) void _start() noexcept {
    using namespace fipsos;

    print_literal(
        "[avatar_cpp.elf] Motor 3D Phong C++20 v1.1 (FP32 IEEE-754 + SSE2 XMM) en Anillo 3 (RIP=0x600000)!\n"
    );

    const int64_t pid = sys_getpid();

    // 1. Dynamic 16-byte-aligned User Heap allocation via sys_brk (12) at 0x0200_0000+
    const uint64_t heap_base = sys_brk(0);
    const uint64_t heap_end  = sys_brk(heap_base + 4096);
    if (heap_end <= heap_base) {
        sys_exit(1);
    }

    auto* state = reinterpret_cast<AvatarHeapState*>(heap_base);
    state->yaw_rad            = 0.35f;
    state->pitch_rad          = -0.22f;
    state->radius_px          = 30.0f;
    state->normal_norm_sample = 0.0f;
    state->phong_peak_sample  = 0.0f;
    state->npu_energy_milli   = 0;
    state->palette_mode       = 0;
    state->frames_rendered    = 0;
    state->npu_inferences     = 0;
    state->pid                = pid;

    // Normalize directional light L = (-0.42f, -0.55f, 0.72f) using hardware SSE2 `sqrtss`
    const float raw_lx  = -0.42f;
    const float raw_ly  = -0.55f;
    const float raw_lz  = 0.72f;
    const float inv_len = 1.0f / sqrtf_sse2(raw_lx * raw_lx + raw_ly * raw_ly + raw_lz * raw_lz);
    state->light_dir[0] = raw_lx * inv_len;
    state->light_dir[1] = raw_ly * inv_len;
    state->light_dir[2] = raw_lz * inv_len;

    for (size_t i = 0; i < 16; ++i) {
        state->fp32_weights[i] = 0.50f;
    }

    // 2. Open AI model & NPU device, Zero-Copy mmap `/dev/accel/accel0`, and run C11 FP32 GEMM
    const int64_t model_fd = sys_open("/models/avatar_v1.bin", O_RDONLY);
    const int64_t accel_fd = sys_open("/dev/accel/accel0", O_RDWR);
    const volatile uint8_t* accel_vmo = nullptr;

    if (accel_fd >= 0) {
        const int64_t mapped_npu = sys_mmap(
            0,
            4096,
            PROT_READ,
            MAP_SHARED,
            static_cast<uint64_t>(accel_fd),
            0
        );
        if (mapped_npu > 0) {
            accel_vmo = reinterpret_cast<const volatile uint8_t*>(mapped_npu);
        }
    }

    if (model_fd >= 0 && accel_fd >= 0) {
        const int64_t energy_milli = sys_ioctl(
            static_cast<uint64_t>(accel_fd),
            ACCEL_IOCTL_RUN_INFERENCE,
            static_cast<uint64_t>(model_fd)
        );
        if (energy_milli >= 0) {
            state->npu_energy_milli = static_cast<uint32_t>(energy_milli);
            state->npu_inferences += 1U;
            sync_npu_fp32_weights(accel_vmo, state);
        }
    }

    // 3. Open and Zero-Copy mmap /dev/fb0
    const int64_t fb_fd = sys_open("/dev/fb0", O_RDWR);
    volatile uint32_t* fb_vmo = nullptr;
    constexpr uint64_t kPackedDims =
        (static_cast<uint64_t>(AvatarEngine3D::kWidth) << 16) |
        static_cast<uint64_t>(AvatarEngine3D::kHeight);

    if (fb_fd >= 0) {
        constexpr size_t kByteLen = AvatarEngine3D::kWidth * AvatarEngine3D::kHeight * 4;
        const int64_t mapped = sys_mmap(
            0,
            kByteLen,
            PROT_READ | PROT_WRITE,
            MAP_SHARED,
            static_cast<uint64_t>(fb_fd),
            0
        );
        if (mapped > 0) {
            fb_vmo = reinterpret_cast<volatile uint32_t*>(mapped);
        }
    }

    auto present_current_frame = [&]() noexcept {
        if (fb_vmo != nullptr && fb_fd >= 0) {
            AvatarEngine3D::render_frame(fb_vmo, state);
            sys_ioctl(static_cast<uint64_t>(fb_fd), FB_IOCTL_PRESENT_RECT, kPackedDims);
        }
    };

    // Render initial 3D Phong sphere + HUD frame and verify FP32 SSE2 math on console
    present_current_frame();
    {
        const float l_norm = sqrtf_sse2(
            state->light_dir[0] * state->light_dir[0] +
            state->light_dir[1] * state->light_dir[1] +
            state->light_dir[2] * state->light_dir[2]
        );
        size_t vlen = 0;
        vlen = append_str(state->fmt_buf, vlen, "[avatar_cpp::sse2] Verificacion FP32 SSE2: |N|=");
        vlen = append_fp32(state->fmt_buf, vlen, state->normal_norm_sample);
        vlen = append_str(state->fmt_buf, vlen, " | |L|=");
        vlen = append_fp32(state->fmt_buf, vlen, l_norm);
        vlen = append_str(state->fmt_buf, vlen, " | NPU_FP32[0]=");
        vlen = append_fp32(state->fmt_buf, vlen, state->fp32_weights[0]);
        vlen = append_str(state->fmt_buf, vlen, " | PhongPeak=");
        vlen = append_fp32(state->fmt_buf, vlen, state->phong_peak_sample);
        vlen = append_str(state->fmt_buf, vlen, "\n");
        sys_write(FD_STDOUT, state->fmt_buf, vlen);
    }

    print_literal(
        "[avatar_cpp::loop] Phong 3D SSE2 activo: [W/A/S/D] Rotar rad/Zoom | [N] GEMM FP32 NPU | [Q] Salir\n"
    );

    // 4. Real-Time Non-Blocking Key Polling Loop (FB_IOCTL_POLL_KEY)
    bool interactive_engaged = false;
    uint32_t idle_polls = 0;
    const uint32_t max_initial_polls = 400;
    const uint32_t max_engaged_polls = 2500;

    while (fb_fd >= 0) {
        const int64_t key = sys_ioctl(static_cast<uint64_t>(fb_fd), FB_IOCTL_POLL_KEY, 0);
        if (key > 0) {
            const char ch = static_cast<char>(key & 0xFF);
            if (ch == 'a' || ch == 'A') {
                interactive_engaged = true;
                idle_polls = 0;
                state->yaw_rad = wrap_pi(state->yaw_rad - 0.28f);
                present_current_frame();
                print_literal("[avatar_cpp::input] Tecla 'A' -> Yaw FP32 (-0.28 rad) en /dev/fb0\n");
            } else if (ch == 'd' || ch == 'D') {
                interactive_engaged = true;
                idle_polls = 0;
                state->yaw_rad = wrap_pi(state->yaw_rad + 0.28f);
                present_current_frame();
                print_literal("[avatar_cpp::input] Tecla 'D' -> Yaw FP32 (+0.28 rad) en /dev/fb0\n");
            } else if (ch == 'w' || ch == 'W') {
                interactive_engaged = true;
                idle_polls = 0;
                state->pitch_rad = wrap_pi(state->pitch_rad - 0.22f);
                if (state->radius_px < 35.0f) {
                    state->radius_px += 1.5f;
                }
                present_current_frame();
                print_literal("[avatar_cpp::input] Tecla 'W' -> Pitch/Zoom FP32 (-0.22 rad) en /dev/fb0\n");
            } else if (ch == 's' || ch == 'S') {
                interactive_engaged = true;
                idle_polls = 0;
                state->pitch_rad = wrap_pi(state->pitch_rad + 0.22f);
                if (state->radius_px > 22.0f) {
                    state->radius_px -= 1.5f;
                }
                present_current_frame();
                print_literal("[avatar_cpp::input] Tecla 'S' -> Pitch/Zoom FP32 (+0.22 rad) en /dev/fb0\n");
            } else if (ch == 'n' || ch == 'N') {
                interactive_engaged = true;
                idle_polls = 0;
                if (model_fd >= 0 && accel_fd >= 0) {
                    const int64_t energy_milli = sys_ioctl(
                        static_cast<uint64_t>(accel_fd),
                        ACCEL_IOCTL_RUN_INFERENCE,
                        static_cast<uint64_t>(model_fd)
                    );
                    if (energy_milli >= 0) {
                        state->npu_energy_milli = static_cast<uint32_t>(energy_milli);
                        sync_npu_fp32_weights(accel_vmo, state);
                    }
                }
                state->palette_mode = (state->palette_mode + 1U) & 3U;
                state->npu_inferences += 1U;
                present_current_frame();
                print_literal(
                    "[avatar_cpp::npu] Tecla 'N' -> Inferencia C11 FP32 GEMM + Phong 3D actualizado!\n"
                );
            } else if (ch == 'q' || ch == 'Q' || ch == 0x1B) {
                print_literal(
                    "[avatar_cpp::input] Tecla 'Q' -> Cerrando Event Loop Phong 3D y liberando descriptores...\n"
                );
                break;
            }
        } else {
            for (uint32_t spin = 0; spin < 25000U; ++spin) {
                asm volatile("pause");
            }
            if ((idle_polls & 31U) == 0U) {
                sys_sched_yield();
            }
            idle_polls += 1;
            const uint32_t limit = interactive_engaged ? max_engaged_polls : max_initial_polls;
            if (idle_polls >= limit) {
                break;
            }
        }
    }

    // 5. Release capability descriptors with POSIX sys_close (3)
    if (fb_fd >= 0) {
        sys_close(static_cast<uint64_t>(fb_fd));
    }
    if (accel_fd >= 0) {
        sys_close(static_cast<uint64_t>(accel_fd));
    }
    if (model_fd >= 0) {
        sys_close(static_cast<uint64_t>(model_fd));
    }

    // 6. Format and print FP32 SSE2 verification summary from Ring 3 User Heap
    size_t len = 0;
    len = append_str(state->fmt_buf, len, "[avatar_cpp.elf] Resumen v1.1 FP32 SSE2: Frames=");
    len = append_u32(state->fmt_buf, len, state->frames_rendered);
    len = append_str(state->fmt_buf, len, " | NPU_FP32[0]=");
    len = append_fp32(state->fmt_buf, len, state->fp32_weights[0]);
    len = append_str(state->fmt_buf, len, " | |N|=");
    len = append_fp32(state->fmt_buf, len, state->normal_norm_sample);
    len = append_str(state->fmt_buf, len, " | Yaw=");
    len = append_fp32(state->fmt_buf, len, state->yaw_rad);
    len = append_str(state->fmt_buf, len, " rad OK!\n");
    sys_write(FD_STDOUT, state->fmt_buf, len);

    sys_exit(0);
}
