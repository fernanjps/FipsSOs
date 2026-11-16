#ifndef FIPSOS_SIMD_NN_RT_H
#define FIPSOS_SIMD_NN_RT_H
#define _MM_MALLOC_H_INCLUDED
#include <immintrin.h>
#include <stdint.h>
#include <unistd.h>
#include <fcntl.h>
#include <time.h>

#define FNN_IN_DIM    4
#define FNN_HID_DIM   8
#define FNN_OUT_DIM   3
#define FNN_GUARD_IN  6
#define FNN_MAGIC     0x314E4E46U /* "FNN1" */

typedef struct {
    uint32_t magic, quantized;
    float w1_scale, w1[FNN_HID_DIM * FNN_IN_DIM], b1[FNN_HID_DIM];
    float w2[FNN_OUT_DIM * FNN_HID_DIM], b2[FNN_OUT_DIM];
    int8_t w1_q[FNN_HID_DIM * FNN_IN_DIM];
} FnnModel;

static FnnModel g_fnn_model;
static float g_fnn_probs[FNN_OUT_DIM];
static int64_t g_fnn_last_us = 1, g_fnn_from_file = 0;

static inline int fnn_has_avx2(void) {
    static int cached = -1;
    if (cached >= 0) return cached;
    uint32_t eax = 0, ebx = 0, ecx = 0, edx = 0;
    __asm__ volatile("cpuid" : "+a"(eax), "=b"(ebx), "=c"(ecx), "=d"(edx));
    if (eax < 7U) { cached = 0; return 0; }
    eax = 1; ecx = 0;
    __asm__ volatile("cpuid" : "+a"(eax), "=b"(ebx), "=c"(ecx), "=d"(edx));
    if ((ecx & ((1U << 27) | (1U << 28))) != ((1U << 27) | (1U << 28))) { cached = 0; return 0; }
    uint32_t xlo = 0, xhi = 0;
    __asm__ volatile("xgetbv" : "=a"(xlo), "=d"(xhi) : "c"(0));
    if ((xlo & 0x06U) != 0x06U) { cached = 0; return 0; }
    eax = 7; ecx = 0;
    __asm__ volatile("cpuid" : "+a"(eax), "=b"(ebx), "+c"(ecx), "=d"(edx));
    cached = ((ebx & (1U << 5)) != 0U) ? 1 : 0;
    return cached;
}

static inline const char* fnn_simd_backend(void) {
    return fnn_has_avx2() ? "AVX2" : "SSE2";
}

static inline float fnn_hsum_ps(__m128 v) {
    __m128 shuf = _mm_shuffle_ps(v, v, _MM_SHUFFLE(2, 3, 0, 1));
    __m128 sums = _mm_add_ps(v, shuf);
    shuf = _mm_movehl_ps(shuf, sums);
    return _mm_cvtss_f32(_mm_add_ss(sums, shuf));
}

__attribute__((target("avx2")))
static void fnn_dense_fp32_avx2(const float* in, const float* w, const float* bias,
                                float* out, int in_dim, int out_dim) {
    for (int o = 0; o < out_dim; ++o) {
        const float* row = w + o * in_dim;
        __m256 acc = _mm256_setzero_ps();
        int i = 0;
        for (; i + 8 <= in_dim; i += 8) {
            acc = _mm256_add_ps(acc, _mm256_mul_ps(_mm256_loadu_ps(in + i), _mm256_loadu_ps(row + i)));
        }
        __m128 lo = _mm256_castps256_ps128(acc);
        __m128 hi = _mm256_extractf128_ps(acc, 1);
        float s = fnn_hsum_ps(_mm_add_ps(lo, hi));
        for (; i < in_dim; ++i) s += in[i] * row[i];
        out[o] = s + (bias ? bias[o] : 0.0f);
    }
}

static void fnn_dense_fp32_sse2(const float* in, const float* w, const float* bias,
                                float* out, int in_dim, int out_dim) {
    for (int o = 0; o < out_dim; ++o) {
        const float* row = w + o * in_dim;
        __m128 acc = _mm_setzero_ps();
        int i = 0;
        for (; i + 4 <= in_dim; i += 4) acc = _mm_add_ps(acc, _mm_mul_ps(_mm_loadu_ps(in + i), _mm_loadu_ps(row + i)));
        float s = fnn_hsum_ps(acc);
        for (; i < in_dim; ++i) s += in[i] * row[i];
        out[o] = s + (bias ? bias[o] : 0.0f);
    }
}

static void fnn_dense_fp32(const float* in, const float* w, const float* bias,
                           float* out, int in_dim, int out_dim) {
    if (fnn_has_avx2()) fnn_dense_fp32_avx2(in, w, bias, out, in_dim, out_dim);
    else fnn_dense_fp32_sse2(in, w, bias, out, in_dim, out_dim);
}

static void fnn_dense_int8_sse2(const float* in, const int8_t* wq, float scale,
                                const float* bias, float* out, int in_dim, int out_dim) {
    const __m128 vsc = _mm_set1_ps(scale);
    for (int o = 0; o < out_dim; ++o) {
        const int8_t* row = wq + o * in_dim;
        __m128 acc = _mm_setzero_ps();
        int i = 0;
        for (; i + 4 <= in_dim; i += 4) {
            __m128 vw = _mm_mul_ps(_mm_set_ps((float)row[i+3], (float)row[i+2], (float)row[i+1], (float)row[i+0]), vsc);
            acc = _mm_add_ps(acc, _mm_mul_ps(_mm_loadu_ps(in + i), vw));
        }
        float s = fnn_hsum_ps(acc);
        for (; i < in_dim; ++i) s += in[i] * ((float)row[i] * scale);
        out[o] = s + (bias ? bias[o] : 0.0f);
    }
}

static void fnn_relu_sse2(float* x, int n) {
    const __m128 z = _mm_setzero_ps();
    int i = 0;
    for (; i + 4 <= n; i += 4) _mm_storeu_ps(x + i, _mm_max_ps(_mm_loadu_ps(x + i), z));
    for (; i < n; ++i) if (x[i] < 0.0f) x[i] = 0.0f;
}

static float fnn_fast_expf(float x) {
    if (x < -20.0f) return 0.0f;
    if (x > 20.0f) x = 20.0f;
    float s = x * 0.0625f, e = 1.0f + s * (1.0f + s * (0.5f + s * (0.16666667f + s * 0.04166667f)));
    e *= e; e *= e; e *= e; e *= e;
    return e;
}

static void fnn_softmax(const float* logits, float* probs, int n) {
    float mx = logits[0], sum = 0.0f;
    for (int i = 1; i < n; ++i) if (logits[i] > mx) mx = logits[i];
    for (int i = 0; i < n; ++i) { probs[i] = fnn_fast_expf(logits[i] - mx); sum += probs[i]; }
    float inv = (sum > 1e-6f) ? (1.0f / sum) : (1.0f / (float)n);
    for (int i = 0; i < n; ++i) probs[i] *= inv;
}

static void fnn_init_default(void) {
    static const float dw1[32] = { -1.4f,-1.0f,-0.4f,-0.2f,-1.1f,-0.8f,-0.3f,-0.1f,1.2f,0.6f,0.5f,0.3f,0.8f,0.9f,0.4f,0.2f,
                                    1.8f,1.2f,0.9f,0.5f,1.5f,1.4f,0.7f,0.6f,-0.5f,0.4f,0.8f,0.5f,0.4f,-0.3f,0.3f,0.6f };
    static const float db1[8] = { 1.1f,0.9f,-0.35f,-0.25f,-1.35f,-1.25f,-0.15f,-0.10f };
    static const float dw2[24] = { 1.4f,1.2f,-0.5f,-0.4f,-1.5f,-1.3f,-0.2f,-0.2f,-0.4f,-0.3f,1.3f,1.2f,
                                  -0.3f,-0.2f,0.6f,0.5f,-1.2f,-1.0f,0.2f,0.2f,1.7f,1.6f,0.4f,0.4f };
    static const float db2[3] = { 0.10f, 0.05f, -0.15f };
    g_fnn_model.magic = FNN_MAGIC; g_fnn_model.quantized = 0; g_fnn_model.w1_scale = 0.02f;
    for (int i = 0; i < 32; ++i) {
        g_fnn_model.w1[i] = dw1[i];
        int q = (int)(dw1[i] / 0.02f);
        if (q > 127) q = 127; else if (q < -127) q = -127;
        g_fnn_model.w1_q[i] = (int8_t)q;
    }
    for (int i = 0; i < 8; ++i) g_fnn_model.b1[i] = db1[i];
    for (int i = 0; i < 24; ++i) g_fnn_model.w2[i] = dw2[i];
    for (int i = 0; i < 3; ++i) g_fnn_model.b2[i] = db2[i];
}

static inline int64_t fnn_load_model(const char* path) {
    fnn_init_default();
    g_fnn_from_file = 0;
    int fd = open(path, O_RDONLY);
    if (fd < 0) return 0;
    FnnModel tmp = {0};
    uint8_t* dst = (uint8_t*)&tmp;
    size_t got = 0;
    while (got < sizeof(tmp)) {
        ssize_t r = read(fd, dst + got, sizeof(tmp) - got);
        if (r <= 0) break;
        got += (size_t)r;
    }
    close(fd);
    if (got >= (sizeof(FnnModel) - sizeof(tmp.w1_q)) && tmp.magic == FNN_MAGIC) {
        g_fnn_model = tmp;
        if (got < sizeof(FnnModel)) {
            float sc = (g_fnn_model.w1_scale > 1e-6f) ? g_fnn_model.w1_scale : 0.02f;
            for (int i = 0; i < 32; ++i) {
                float qf = g_fnn_model.w1[i] / sc;
                if (!(qf >= -127.0f)) qf = -127.0f;
                else if (qf > 127.0f) qf = 127.0f;
                g_fnn_model.w1_q[i] = (int8_t)(int)qf;
            }
        }
        g_fnn_from_file = 1;
        return 1;
    }
    return 0;
}

static inline int64_t fnn_run_telemetry(int64_t cpu, int64_t mem, int64_t procs, int64_t io_kb) {
    struct timespec t0, t1;
    clock_gettime(CLOCK_MONOTONIC, &t0);
    float in[FNN_IN_DIM] = { (float)cpu * 0.01f, (float)mem * 0.01f, (float)procs * 0.05f, (float)io_kb * 0.001f };
    float hid[FNN_HID_DIM], logits[FNN_OUT_DIM];
    if (g_fnn_model.quantized) fnn_dense_int8_sse2(in, g_fnn_model.w1_q, g_fnn_model.w1_scale, g_fnn_model.b1, hid, FNN_IN_DIM, FNN_HID_DIM);
    else fnn_dense_fp32(in, g_fnn_model.w1, g_fnn_model.b1, hid, FNN_IN_DIM, FNN_HID_DIM);
    fnn_relu_sse2(hid, FNN_HID_DIM);
    fnn_dense_fp32(hid, g_fnn_model.w2, g_fnn_model.b2, logits, FNN_HID_DIM, FNN_OUT_DIM);
    fnn_softmax(logits, g_fnn_probs, FNN_OUT_DIM);
    clock_gettime(CLOCK_MONOTONIC, &t1);
    int64_t dt_us = ((t1.tv_sec - t0.tv_sec) * 1000000LL) + ((t1.tv_nsec - t0.tv_nsec) / 1000LL);
    g_fnn_last_us = (dt_us > 0) ? dt_us : 1;
    int best = 0;
    for (int i = 1; i < FNN_OUT_DIM; ++i) if (g_fnn_probs[i] > g_fnn_probs[best]) best = i;
    return (int64_t)best;
}

static int64_t fnn_guard_infer(int64_t http_rate, int64_t traversal, int64_t err_403_404,
                               int64_t sock_opens, int64_t out_writes, int64_t slot_pct) {
    static const float gw1[FNN_HID_DIM * FNN_GUARD_IN] = {
        -0.6f,-2.5f,-1.2f,-0.4f,-2.0f,-0.3f,  -0.4f,-2.0f,-1.0f,-0.3f,-1.6f,-0.2f,
         0.9f, 0.4f, 1.3f, 0.7f, 0.4f, 0.5f,   0.7f, 0.3f, 1.1f, 0.6f, 0.3f, 0.4f,
         1.2f, 3.8f, 1.8f, 0.8f, 3.2f, 0.7f,   1.0f, 3.4f, 1.6f, 0.7f, 2.8f, 0.6f,
         0.5f, 2.6f, 1.2f, 0.5f, 2.4f, 0.4f,  -0.2f, 0.5f, 0.8f, 0.4f, 0.6f, 0.3f
    };
    static const float gb1[FNN_HID_DIM] = { 1.4f, 1.1f, -0.5f, -0.4f, -1.2f, -1.0f, -0.8f, -0.2f };
    static const float gw2[FNN_OUT_DIM * FNN_HID_DIM] = {
         1.6f, 1.3f,-0.6f,-0.5f,-1.8f,-1.6f,-1.3f,-0.3f,
        -0.5f,-0.4f, 1.4f, 1.2f,-0.3f,-0.2f,-0.2f, 0.6f,
        -1.5f,-1.2f, 0.2f, 0.2f, 2.1f, 1.9f, 1.6f, 0.4f
    };
    static const float gb2[FNN_OUT_DIM] = { 0.15f, 0.0f, -0.25f };
    struct timespec t0, t1;
    clock_gettime(CLOCK_MONOTONIC, &t0);
    float in[FNN_GUARD_IN] = { (float)http_rate * 0.02f, (float)traversal * 0.5f, (float)err_403_404 * 0.2f,
                               (float)sock_opens * 0.05f, (float)out_writes * 0.5f, (float)slot_pct * 0.01f };
    float hid[FNN_HID_DIM], logits[FNN_OUT_DIM];
    fnn_dense_fp32(in, gw1, gb1, hid, FNN_GUARD_IN, FNN_HID_DIM);
    fnn_relu_sse2(hid, FNN_HID_DIM);
    fnn_dense_fp32(hid, gw2, gb2, logits, FNN_HID_DIM, FNN_OUT_DIM);
    fnn_softmax(logits, g_fnn_probs, FNN_OUT_DIM);
    clock_gettime(CLOCK_MONOTONIC, &t1);
    int64_t dt_us = ((t1.tv_sec - t0.tv_sec) * 1000000LL) + ((t1.tv_nsec - t0.tv_nsec) / 1000LL);
    g_fnn_last_us = (dt_us > 0) ? dt_us : 1;
    int best = 0;
    for (int i = 1; i < FNN_OUT_DIM; ++i) if (g_fnn_probs[i] > g_fnn_probs[best]) best = i;
    return (int64_t)best;
}

static inline int64_t fnn_prob_pct(int64_t idx) { return (idx >= 0 && idx < FNN_OUT_DIM) ? (int64_t)(g_fnn_probs[idx] * 100.0f + 0.5f) : 0; }
static inline int64_t fnn_last_us(void) { return g_fnn_last_us; }
static inline void fnn_set_quantized(int64_t q) { g_fnn_model.quantized = q ? 1U : 0U; }
#endif
