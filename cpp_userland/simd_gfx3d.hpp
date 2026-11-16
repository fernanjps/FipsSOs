#pragma once

#define _MM_MALLOC_H_INCLUDED
#include <emmintrin.h>
#include <stdint.h>

namespace fipsos::gfx3d {

struct Vec4 {
    float x, y, z, w;
};

struct alignas(16) Mat4 {
    float m[16];
};

inline float sqrt_sse2(float x) noexcept {
    if (x <= 0.0f) return 0.0f;
    float out = 0.0f;
    __m128 v = _mm_set_ss(x);
    _mm_store_ss(&out, _mm_sqrt_ss(v));
    return out;
}

inline float clamp01(float v) noexcept {
    return (v < 0.0f) ? 0.0f : ((v > 1.0f) ? 1.0f : v);
}

inline float wrap_pi(float r) noexcept {
    constexpr float kPi = 3.14159265f;
    constexpr float kTwoPi = 6.28318531f;
    while (r > kPi) r -= kTwoPi;
    while (r < -kPi) r += kTwoPi;
    return r;
}

inline float sinf_fast(float r) noexcept {
    constexpr float kPi = 3.14159265f;
    const float x = wrap_pi(r);
    const float ax = (x >= 0.0f) ? x : -x;
    const float y = (4.0f / kPi) * x + (-4.0f / (kPi * kPi)) * x * ax;
    const float ay = (y >= 0.0f) ? y : -y;
    return 0.225f * (y * ay - y) + y;
}

inline float cosf_fast(float r) noexcept {
    return sinf_fast(r + 1.57079633f);
}

/// SSE2 4x4 FP32 matrix multiplication: `out = a * b`.
inline Mat4 mat4_mul_sse2(const Mat4& a, const Mat4& b) noexcept {
    Mat4 out{};
    const __m128 b0 = _mm_loadu_ps(&b.m[0]);
    const __m128 b1 = _mm_loadu_ps(&b.m[4]);
    const __m128 b2 = _mm_loadu_ps(&b.m[8]);
    const __m128 b3 = _mm_loadu_ps(&b.m[12]);
    for (int row = 0; row < 4; ++row) {
        const float* ar = &a.m[row * 4];
        __m128 acc = _mm_mul_ps(_mm_set1_ps(ar[0]), b0);
        acc = _mm_add_ps(acc, _mm_mul_ps(_mm_set1_ps(ar[1]), b1));
        acc = _mm_add_ps(acc, _mm_mul_ps(_mm_set1_ps(ar[2]), b2));
        acc = _mm_add_ps(acc, _mm_mul_ps(_mm_set1_ps(ar[3]), b3));
        _mm_storeu_ps(&out.m[row * 4], acc);
    }
    return out;
}

/// SSE2 4x4 matrix * Vec4 column-vector product.
inline Vec4 mat4_mul_vec4_sse2(const Mat4& mat, const Vec4& v) noexcept {
    const __m128 vec = _mm_set_ps(v.w, v.z, v.y, v.x);
    float tmp[4];
    for (int row = 0; row < 4; ++row) {
        const __m128 mr = _mm_loadu_ps(&mat.m[row * 4]);
        const __m128 prod = _mm_mul_ps(mr, vec);
        const __m128 sh1 = _mm_shuffle_ps(prod, prod, _MM_SHUFFLE(2, 3, 0, 1));
        const __m128 s1 = _mm_add_ps(prod, sh1);
        const __m128 sh2 = _mm_shuffle_ps(s1, s1, _MM_SHUFFLE(1, 0, 3, 2));
        const __m128 s2 = _mm_add_ps(s1, sh2);
        _mm_store_ss(&tmp[row], s2);
    }
    return Vec4{tmp[0], tmp[1], tmp[2], tmp[3]};
}

/// Verifiable FP32 4x4 tensor matmul kernel using SSE2. Returns trace checksum.
inline float tensor_matmul_4x4_sse2(const float* a16, const float* b16, float* out16) noexcept {
    Mat4 a{}, b{};
    for (int i = 0; i < 16; ++i) {
        a.m[i] = a16[i];
        b.m[i] = b16[i];
    }
    const Mat4 c = mat4_mul_sse2(a, b);
    for (int i = 0; i < 16; ++i) {
        out16[i] = c.m[i];
    }
    return c.m[0] + c.m[5] + c.m[10] + c.m[15];
}

inline Mat4 mat4_rot_y(float rad) noexcept {
    const float c = cosf_fast(rad);
    const float s = sinf_fast(rad);
    return Mat4{{
        c,    0.0f, s,    0.0f,
        0.0f, 1.0f, 0.0f, 0.0f,
       -s,    0.0f, c,    0.0f,
        0.0f, 0.0f, 0.0f, 1.0f
    }};
}

inline Mat4 mat4_rot_x(float rad) noexcept {
    const float c = cosf_fast(rad);
    const float s = sinf_fast(rad);
    return Mat4{{
        1.0f, 0.0f, 0.0f, 0.0f,
        0.0f, c,   -s,    0.0f,
        0.0f, s,    c,    0.0f,
        0.0f, 0.0f, 0.0f, 1.0f
    }};
}

inline float edge_fn(float ax, float ay, float bx, float by, float cx, float cy) noexcept {
    return (cx - ax) * (by - ay) - (cy - ay) * (bx - ax);
}

inline void raster_tri_zbuf(
    uint32_t* tile,
    float* zbuf,
    int size,
    int max_pixels,
    const Vec4& v0,
    const Vec4& v1,
    const Vec4& v2,
    uint32_t color
) noexcept {
    const float area = edge_fn(v0.x, v0.y, v1.x, v1.y, v2.x, v2.y);
    if (area <= 0.5f) return;
    const float inv_area = 1.0f / area;

    int min_x = static_cast<int>((v0.x < v1.x) ? ((v0.x < v2.x) ? v0.x : v2.x) : ((v1.x < v2.x) ? v1.x : v2.x));
    int max_x = static_cast<int>((v0.x > v1.x) ? ((v0.x > v2.x) ? v0.x : v2.x) : ((v1.x > v2.x) ? v1.x : v2.x)) + 1;
    int min_y = static_cast<int>((v0.y < v1.y) ? ((v0.y < v2.y) ? v0.y : v2.y) : ((v1.y < v2.y) ? v1.y : v2.y));
    int max_y = static_cast<int>((v0.y > v1.y) ? ((v0.y > v2.y) ? v0.y : v2.y) : ((v1.y > v2.y) ? v1.y : v2.y)) + 1;

    if (min_x < 0) min_x = 0;
    if (min_y < 0) min_y = 0;
    if (max_x > size) max_x = size;
    if (max_y > size) max_y = size;

    for (int y = min_y; y < max_y; ++y) {
        const float py = static_cast<float>(y) + 0.5f;
        for (int x = min_x; x < max_x; ++x) {
            const float px = static_cast<float>(x) + 0.5f;
            const float w0 = edge_fn(v1.x, v1.y, v2.x, v2.y, px, py);
            const float w1 = edge_fn(v2.x, v2.y, v0.x, v0.y, px, py);
            const float w2 = edge_fn(v0.x, v0.y, v1.x, v1.y, px, py);
            if (w0 >= 0.0f && w1 >= 0.0f && w2 >= 0.0f) {
                const float z = (w0 * v0.z + w1 * v1.z + w2 * v2.z) * inv_area;
                const int idx = y * size + x;
                if (idx < max_pixels && z < zbuf[idx]) {
                    zbuf[idx] = z;
                    tile[idx] = color;
                }
            }
        }
    }
}

/// Renders an SSE2-transformed, Z-buffered 3D octahedral crystal + orbiting prism mesh into `tile`.
inline void render_scene_zbuf_sse2(
    uint32_t* tile,
    int size,
    float yaw,
    float pitch,
    const float light_dir[3],
    uint32_t palette_mode,
    float* out_simd_trace
) noexcept {
    constexpr int kMaxPixels = 160 * 160;
    static float zbuf[kMaxPixels];
    const int total = size * size;
    for (int y = 0; y < size; ++y) {
        for (int x = 0; x < size; ++x) {
            const int idx = y * size + x;
            if (idx < kMaxPixels) zbuf[idx] = 1e9f;
            const bool grid = ((x & 15) == 0) || ((y & 15) == 0);
            tile[idx] = grid ? 0x0E1F38U : 0x081224U;
        }
    }

    const Mat4 rot = mat4_mul_sse2(mat4_rot_x(pitch), mat4_rot_y(yaw));
    if (out_simd_trace != nullptr) {
        float tmp[16];
        const Mat4 inv_rot = mat4_mul_sse2(mat4_rot_y(-yaw), mat4_rot_x(-pitch));
        *out_simd_trace = tensor_matmul_4x4_sse2(rot.m, inv_rot.m, tmp);
    }

    // Primary octahedron (6 vertices, 8 triangles) + orbiting secondary tetrahedron (4 vertices, 4 triangles)
    static constexpr Vec4 kVerts[10] = {
        { 0.0f,  1.05f,  0.0f, 1.0f}, // 0 top
        { 0.95f, 0.0f,   0.0f, 1.0f}, // 1 right
        { 0.0f,  0.0f,   0.95f,1.0f}, // 2 front
        {-0.95f, 0.0f,   0.0f, 1.0f}, // 3 left
        { 0.0f,  0.0f,  -0.95f,1.0f}, // 4 back
        { 0.0f, -1.05f,  0.0f, 1.0f}, // 5 bottom
        // Orbiting satellite prism
        { 1.25f, 0.55f,  0.25f,1.0f}, // 6
        { 1.55f, 0.10f,  0.55f,1.0f}, // 7
        { 1.00f, 0.10f,  0.65f,1.0f}, // 8
        { 1.28f, 0.15f, -0.10f,1.0f}, // 9
    };
    static constexpr int kTris[12][3] = {
        {0, 2, 1}, {0, 1, 4}, {0, 4, 3}, {0, 3, 2},
        {5, 1, 2}, {5, 4, 1}, {5, 3, 4}, {5, 2, 3},
        {6, 8, 7}, {6, 7, 9}, {6, 9, 8}, {7, 8, 9},
    };

    Vec4 world[10];
    Vec4 screen[10];
    const float half = static_cast<float>(size) * 0.5f;
    const float scale = static_cast<float>(size) * 0.34f;

    for (int i = 0; i < 10; ++i) {
        const Vec4 p = mat4_mul_vec4_sse2(rot, kVerts[i]);
        world[i] = p;
        const float persp = 3.4f / (3.4f - p.z);
        screen[i] = Vec4{
            half + p.x * scale * persp,
            half - p.y * scale * persp,
            3.4f - p.z,
            1.0f
        };
    }

    float base_r = 0.12f, base_g = 0.78f, base_b = 0.96f;
    if ((palette_mode & 3U) == 1U) {
        base_r = 0.14f; base_g = 0.90f; base_b = 0.56f;
    } else if ((palette_mode & 3U) == 2U) {
        base_r = 0.80f; base_g = 0.40f; base_b = 0.98f;
    } else if ((palette_mode & 3U) == 3U) {
        base_r = 0.98f; base_g = 0.66f; base_b = 0.18f;
    }

    for (int t = 0; t < 12; ++t) {
        const Vec4& a = world[kTris[t][0]];
        const Vec4& b = world[kTris[t][1]];
        const Vec4& c = world[kTris[t][2]];
        const float ux = b.x - a.x, uy = b.y - a.y, uz = b.z - a.z;
        const float vx = c.x - a.x, vy = c.y - a.y, vz = c.z - a.z;
        float nx = uy * vz - uz * vy;
        float ny = uz * vx - ux * vz;
        float nz = ux * vy - uy * vx;
        const float len = sqrt_sse2(nx * nx + ny * ny + nz * nz);
        if (len > 1e-5f) {
            const float inv = 1.0f / len;
            nx *= inv; ny *= inv; nz *= inv;
        }
        const float ndotl = clamp01(nx * light_dir[0] - ny * light_dir[1] + nz * light_dir[2]);
        const float lit = 0.20f + 0.80f * ndotl;
        const float sat_tint = (t >= 8) ? 1.15f : 1.0f;
        const uint32_t cr = static_cast<uint32_t>(clamp01(base_r * lit * sat_tint) * 255.0f);
        const uint32_t cg = static_cast<uint32_t>(clamp01(base_g * lit * (t >= 8 ? 0.85f : 1.0f)) * 255.0f);
        const uint32_t cb = static_cast<uint32_t>(clamp01(base_b * lit) * 255.0f);
        const uint32_t col = (cr << 16) | (cg << 8) | cb;
        raster_tri_zbuf(tile, zbuf, size, kMaxPixels, screen[kTris[t][0]], screen[kTris[t][1]], screen[kTris[t][2]], col);
    }
    (void)total;
}

} // namespace fipsos::gfx3d
