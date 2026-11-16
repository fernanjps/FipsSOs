#pragma once

#define _MM_MALLOC_H_INCLUDED
#include <emmintrin.h>
#include <stddef.h>
#include <stdint.h>

namespace fipsos::nn {

constexpr int kInDim  = 4;
constexpr int kHidDim = 8;
constexpr int kOutDim = 3;

struct MlpWeights {
    float  w1[kHidDim * kInDim];
    float  b1[kHidDim];
    float  w2[kOutDim * kHidDim];
    float  b2[kOutDim];
    int8_t w1_q[kHidDim * kInDim];
    float  w1_scale;
    bool   quantized;
};

inline float hsum_ps(__m128 v) noexcept {
    __m128 shuf = _mm_shuffle_ps(v, v, _MM_SHUFFLE(2, 3, 0, 1));
    __m128 sums = _mm_add_ps(v, shuf);
    shuf        = _mm_movehl_ps(shuf, sums);
    sums        = _mm_add_ss(sums, shuf);
    return _mm_cvtss_f32(sums);
}

inline void dense_fp32_sse2(const float* in, const float* w, const float* bias,
                            float* out, int in_dim, int out_dim) noexcept {
    for (int o = 0; o < out_dim; ++o) {
        const float* row = w + o * in_dim;
        __m128 acc = _mm_setzero_ps();
        int i = 0;
        for (; i + 4 <= in_dim; i += 4) {
            __m128 vx = _mm_loadu_ps(in + i);
            __m128 vw = _mm_loadu_ps(row + i);
            acc = _mm_add_ps(acc, _mm_mul_ps(vx, vw));
        }
        float sum = hsum_ps(acc);
        for (; i < in_dim; ++i) {
            sum += in[i] * row[i];
        }
        out[o] = sum + (bias ? bias[o] : 0.0f);
    }
}

inline void dense_int8_sse2(const float* in, const int8_t* w_q, float w_scale,
                            const float* bias, float* out,
                            int in_dim, int out_dim) noexcept {
    const __m128 vscale = _mm_set1_ps(w_scale);
    for (int o = 0; o < out_dim; ++o) {
        const int8_t* row = w_q + o * in_dim;
        __m128 acc = _mm_setzero_ps();
        int i = 0;
        for (; i + 4 <= in_dim; i += 4) {
            __m128 vx = _mm_loadu_ps(in + i);
            __m128 vw = _mm_set_ps(
                static_cast<float>(row[i + 3]),
                static_cast<float>(row[i + 2]),
                static_cast<float>(row[i + 1]),
                static_cast<float>(row[i + 0]));
            vw = _mm_mul_ps(vw, vscale);
            acc = _mm_add_ps(acc, _mm_mul_ps(vx, vw));
        }
        float sum = hsum_ps(acc);
        for (; i < in_dim; ++i) {
            sum += in[i] * (static_cast<float>(row[i]) * w_scale);
        }
        out[o] = sum + (bias ? bias[o] : 0.0f);
    }
}

inline void relu_sse2(float* x, int n) noexcept {
    const __m128 zero = _mm_setzero_ps();
    int i = 0;
    for (; i + 4 <= n; i += 4) {
        __m128 v = _mm_loadu_ps(x + i);
        _mm_storeu_ps(x + i, _mm_max_ps(v, zero));
    }
    for (; i < n; ++i) {
        if (x[i] < 0.0f) x[i] = 0.0f;
    }
}

inline float fast_expf(float x) noexcept {
    if (x < -20.0f) return 0.0f;
    if (x > 20.0f) x = 20.0f;
    // 4th-order Taylor after range halving (4 squarings)
    float s = x * 0.0625f;
    float e = 1.0f + s * (1.0f + s * (0.5f + s * (0.16666667f + s * 0.04166667f)));
    e = e * e;
    e = e * e;
    e = e * e;
    e = e * e;
    return e;
}

inline void softmax_stable(const float* logits, float* probs, int n) noexcept {
    float mx = logits[0];
    for (int i = 1; i < n; ++i) {
        if (logits[i] > mx) mx = logits[i];
    }
    float sum = 0.0f;
    for (int i = 0; i < n; ++i) {
        probs[i] = fast_expf(logits[i] - mx);
        sum += probs[i];
    }
    const float inv = (sum > 1e-6f) ? (1.0f / sum) : (1.0f / static_cast<float>(n));
    for (int i = 0; i < n; ++i) {
        probs[i] *= inv;
    }
}

inline MlpWeights default_telemetry_weights() noexcept {
    MlpWeights m = {
        // w1: [8 x 4] -> inputs: [cpu, mem, procs, io]
        {
            -1.4f, -1.0f, -0.4f, -0.2f, // h0: idle detector
            -1.1f, -0.8f, -0.3f, -0.1f, // h1: idle support
             1.2f,  0.6f,  0.5f,  0.3f, // h2: balanced/active
             0.8f,  0.9f,  0.4f,  0.2f, // h3: balanced memory
             1.8f,  1.2f,  0.9f,  0.5f, // h4: high load
             1.5f,  1.4f,  0.7f,  0.6f, // h5: high load 2
            -0.5f,  0.4f,  0.8f,  0.5f, // h6: task burst
             0.4f, -0.3f,  0.3f,  0.6f  // h7: io activity
        },
        // b1: [8]
        { 1.1f, 0.9f, -0.35f, -0.25f, -1.35f, -1.25f, -0.15f, -0.10f },
        // w2: [3 x 8] -> [IDLE, BALANCED, HIGH_LOAD]
        {
             1.4f,  1.2f, -0.5f, -0.4f, -1.5f, -1.3f, -0.2f, -0.2f,
            -0.4f, -0.3f,  1.3f,  1.2f, -0.3f, -0.2f,  0.6f,  0.5f,
            -1.2f, -1.0f,  0.2f,  0.2f,  1.7f,  1.6f,  0.4f,  0.4f
        },
        // b2: [3]
        { 0.10f, 0.05f, -0.15f },
        // w1_q
        {0},
        0.02f,
        false
    };
    for (int i = 0; i < kHidDim * kInDim; ++i) {
        int q = static_cast<int>(m.w1[i] / m.w1_scale);
        if (q > 127) q = 127;
        if (q < -127) q = -127;
        m.w1_q[i] = static_cast<int8_t>(q);
    }
    return m;
}

inline int infer_telemetry(const MlpWeights& net,
                           float cpu_pct, float mem_pct, float procs, float io_kb,
                           float* out_probs) noexcept {
    float in[kInDim] = {
        cpu_pct * 0.01f,
        mem_pct * 0.01f,
        procs * 0.05f,
        io_kb * 0.001f
    };
    float hid[kHidDim];
    float logits[kOutDim];
    if (net.quantized) {
        dense_int8_sse2(in, net.w1_q, net.w1_scale, net.b1, hid, kInDim, kHidDim);
    } else {
        dense_fp32_sse2(in, net.w1, net.b1, hid, kInDim, kHidDim);
    }
    relu_sse2(hid, kHidDim);
    dense_fp32_sse2(hid, net.w2, net.b2, logits, kHidDim, kOutDim);
    softmax_stable(logits, out_probs, kOutDim);
    int best = 0;
    for (int i = 1; i < kOutDim; ++i) {
        if (out_probs[i] > out_probs[best]) best = i;
    }
    return best;
}

inline const char* class_label(int cls) noexcept {
    if (cls == 0) return "IDLE";
    if (cls == 1) return "BALANCED";
    return "HIGH_LOAD";
}

inline bool has_avx2() noexcept {
    uint32_t eax = 1, ebx = 0, ecx = 0, edx = 0;
    asm volatile("cpuid" : "+a"(eax), "=b"(ebx), "=c"(ecx), "=d"(edx));
    if ((ecx & ((1U << 27) | (1U << 28))) != ((1U << 27) | (1U << 28))) return false;
    uint32_t xlo = 0, xhi = 0;
    asm volatile("xgetbv" : "=a"(xlo), "=d"(xhi) : "c"(0));
    if ((xlo & 0x06U) != 0x06U) return false;
    eax = 7; ecx = 0;
    asm volatile("cpuid" : "+a"(eax), "=b"(ebx), "+c"(ecx), "=d"(edx));
    return (ebx & (1U << 5)) != 0U;
}

inline const char* simd_backend_name() noexcept {
    return has_avx2() ? "AVX2" : "SSE2";
}

inline int guard_infer(float http_rate, float traversal, float err403,
                       float socks, float out_writes, float slot_pct,
                       float* out_probs) noexcept {
    static constexpr int kGIn = 6;
    static constexpr float gw1[kHidDim * kGIn] = {
        -0.6f,-2.5f,-1.2f,-0.4f,-2.0f,-0.3f,  -0.4f,-2.0f,-1.0f,-0.3f,-1.6f,-0.2f,
         0.9f, 0.4f, 1.3f, 0.7f, 0.4f, 0.5f,   0.7f, 0.3f, 1.1f, 0.6f, 0.3f, 0.4f,
         1.2f, 3.8f, 1.8f, 0.8f, 3.2f, 0.7f,   1.0f, 3.4f, 1.6f, 0.7f, 2.8f, 0.6f,
         0.5f, 2.6f, 1.2f, 0.5f, 2.4f, 0.4f,  -0.2f, 0.5f, 0.8f, 0.4f, 0.6f, 0.3f
    };
    static constexpr float gb1[kHidDim] = { 1.4f, 1.1f, -0.5f, -0.4f, -1.2f, -1.0f, -0.8f, -0.2f };
    static constexpr float gw2[kOutDim * kHidDim] = {
         1.6f, 1.3f,-0.6f,-0.5f,-1.8f,-1.6f,-1.3f,-0.3f,
        -0.5f,-0.4f, 1.4f, 1.2f,-0.3f,-0.2f,-0.2f, 0.6f,
        -1.5f,-1.2f, 0.2f, 0.2f, 2.1f, 1.9f, 1.6f, 0.4f
    };
    static constexpr float gb2[kOutDim] = { 0.15f, 0.0f, -0.25f };
    float in[kGIn] = {
        http_rate * 0.02f, traversal * 0.5f, err403 * 0.2f,
        socks * 0.05f, out_writes * 0.5f, slot_pct * 0.01f
    };
    float hid[kHidDim], logits[kOutDim];
    dense_fp32_sse2(in, gw1, gb1, hid, kGIn, kHidDim);
    relu_sse2(hid, kHidDim);
    dense_fp32_sse2(hid, gw2, gb2, logits, kHidDim, kOutDim);
    softmax_stable(logits, out_probs, kOutDim);
    int best = 0;
    for (int i = 1; i < kOutDim; ++i) if (out_probs[i] > out_probs[best]) best = i;
    return best;
}

inline const char* guard_label(int cls) noexcept {
    if (cls == 0) return "SAFE";
    if (cls == 1) return "SUSPICIOUS";
    return "INTRUSION";
}

inline void format_guard_state(const char* raw, int raw_len, char* out, int cap) noexcept {
    char verdict[16] = "SAFE";
    char backend[12] = "";
    char blocks[12]  = "0";
    const char* def_be = simd_backend_name();
    int bi = 0;
    for (; def_be[bi] != '\0' && bi < 10; ++bi) backend[bi] = def_be[bi];
    backend[bi] = '\0';
    if (raw != nullptr && raw_len > 3) {
        int part = 0, pi = 0;
        for (int i = 0; i < raw_len && raw[i] != '\0' && raw[i] != '\n' && raw[i] != '\r'; ++i) {
            if (raw[i] == '|') { ++part; pi = 0; continue; }
            if (part == 0 && pi < 15) { verdict[pi++] = raw[i]; verdict[pi] = '\0'; }
            else if (part == 1 && pi < 11) { backend[pi++] = raw[i]; backend[pi] = '\0'; }
            else if (part == 2 && pi < 11) { blocks[pi++] = raw[i]; blocks[pi] = '\0'; }
        }
    }
    int o = 0;
    auto push = [&](const char* s) { while (*s != '\0' && o + 1 < cap) out[o++] = *s++; };
    push(verdict); push("("); push(backend); push(",B:"); push(blocks); push(")");
    out[o] = '\0';
}

} // namespace fipsos::nn
