#ifndef FIPSOS_SIMD_SEQ_NN_H
#define FIPSOS_SIMD_SEQ_NN_H
/*
 * Sequential GRU + Lightweight Attention Engine (AVX2 / SSE2, Zero-Heap)
 * Loads FNN2 binary weights from disk and processes token sequences deterministically.
 */
#include "simd_nn_rt.h"
#include "fnn2_dyn.h"

#define SEQ_MAX_TOKENS 32
#define SEQ_MAX_DIM    16

typedef struct {
    int64_t last_us;
    uint32_t hid_dim;
    uint32_t out_dim;
    float hidden[SEQ_MAX_DIM];
    float probs[SEQ_MAX_DIM];
    int top_idx[3];
    float top_prob[3];
} SeqNnResult;

__attribute__((target("avx2")))
static float seq_dot_avx2(const float* a, const float* b, int n) {
    __m256 acc = _mm256_setzero_ps();
    int i = 0;
    for (; i + 8 <= n; i += 8) {
        acc = _mm256_add_ps(acc, _mm256_mul_ps(_mm256_loadu_ps(a + i), _mm256_loadu_ps(b + i)));
    }
    __m128 lo = _mm256_castps256_ps128(acc);
    __m128 hi = _mm256_extractf128_ps(acc, 1);
    float s = fnn_hsum_ps(_mm_add_ps(lo, hi));
    for (; i < n; ++i) s += a[i] * b[i];
    return s;
}

static float seq_dot_sse2(const float* a, const float* b, int n) {
    __m128 acc = _mm_setzero_ps();
    int i = 0;
    for (; i + 4 <= n; i += 4) {
        acc = _mm_add_ps(acc, _mm_mul_ps(_mm_loadu_ps(a + i), _mm_loadu_ps(b + i)));
    }
    float s = fnn_hsum_ps(acc);
    for (; i < n; ++i) s += a[i] * b[i];
    return s;
}

static inline float seq_dot(const float* a, const float* b, int n) {
    return fnn_has_avx2() ? seq_dot_avx2(a, b, n) : seq_dot_sse2(a, b, n);
}

static inline float seq_sigmoid(float x) {
    if (x < -12.0f) return 0.0f;
    if (x > 12.0f) return 1.0f;
    return 1.0f / (1.0f + fnn_fast_expf(-x));
}

static inline float seq_tanh(float x) {
    if (x < -10.0f) return -1.0f;
    if (x > 10.0f) return 1.0f;
    float e2 = fnn_fast_expf(2.0f * x);
    return (e2 - 1.0f) / (e2 + 1.0f);
}

static int seq_nn_infer(const Fnn2Model* m, const int* tokens, int n_tok, SeqNnResult* res) {
    if (!m || !tokens || n_tok <= 0 || !res) return -1;
    if (n_tok > SEQ_MAX_TOKENS) n_tok = SEQ_MAX_TOKENS;

    uint32_t in_dim = (m->hdr.in_dim <= SEQ_MAX_DIM) ? m->hdr.in_dim : SEQ_MAX_DIM;
    uint32_t hid_dim = (m->hdr.hid_dim <= SEQ_MAX_DIM) ? m->hdr.hid_dim : SEQ_MAX_DIM;
    uint32_t out_dim = (m->hdr.out_dim <= SEQ_MAX_DIM) ? m->hdr.out_dim : SEQ_MAX_DIM;

    static float h_seq[SEQ_MAX_TOKENS][SEQ_MAX_DIM];
    float h[SEQ_MAX_DIM] = {0};
    float x[SEQ_MAX_DIM] = {0};

    struct timespec t0, t1;
    clock_gettime(CLOCK_MONOTONIC, &t0);

    for (int t = 0; t < n_tok; ++t) {
        int tok = tokens[t];
        for (uint32_t i = 0; i < in_dim; ++i) {
            uint32_t mix = (uint32_t)(tok * 2654435761U) ^ (i * 2246822519U);
            x[i] = ((float)(mix & 0xFFU) / 127.5f) - 1.0f;
        }
        float h_next[SEQ_MAX_DIM] = {0};
        for (uint32_t j = 0; j < hid_dim; ++j) {
            const float* w_row = m->w1 + j * m->hdr.in_dim;
            float pre_x = seq_dot(x, w_row, (int)in_dim) + m->b1[j];
            float rec = 0.0f;
            for (uint32_t k = 0; k < hid_dim; ++k) {
                float rw = m->w1[((j + k) % m->hdr.hid_dim) * m->hdr.in_dim + (k % m->hdr.in_dim)];
                rec += h[k] * rw * 0.25f;
            }
            float z = seq_sigmoid(pre_x + rec);
            float r = seq_sigmoid(pre_x * 0.5f - rec);
            float h_cand = seq_tanh(pre_x + r * rec);
            h_next[j] = (1.0f - z) * h[j] + z * h_cand;
        }
        for (uint32_t j = 0; j < hid_dim; ++j) {
            h[j] = h_next[j];
            h_seq[t][j] = h_next[j];
        }
    }

    /* Lightweight scaled dot-product attention over hidden states */
    float attn_scores[SEQ_MAX_TOKENS];
    float attn_probs[SEQ_MAX_TOKENS];
    float inv_sqrt_d = 1.0f / (float)(hid_dim > 1 ? hid_dim : 1);
    for (int t = 0; t < n_tok; ++t) {
        attn_scores[t] = seq_dot(h, h_seq[t], (int)hid_dim) * inv_sqrt_d;
    }
    fnn_softmax(attn_scores, attn_probs, n_tok);

    float ctx[SEQ_MAX_DIM] = {0};
    for (int t = 0; t < n_tok; ++t) {
        for (uint32_t j = 0; j < hid_dim; ++j) {
            ctx[j] += attn_probs[t] * h_seq[t][j];
        }
    }

    float logits[SEQ_MAX_DIM] = {0};
    for (uint32_t o = 0; o < out_dim; ++o) {
        logits[o] = seq_dot(ctx, m->w2 + o * m->hdr.hid_dim, (int)hid_dim) + m->b2[o];
    }
    fnn_softmax(logits, res->probs, (int)out_dim);

    clock_gettime(CLOCK_MONOTONIC, &t1);
    int64_t dt = (t1.tv_sec - t0.tv_sec) * 1000000LL + (t1.tv_nsec - t0.tv_nsec) / 1000LL;
    res->last_us = (dt > 0) ? dt : 1;
    res->hid_dim = hid_dim;
    res->out_dim = out_dim;
    for (uint32_t j = 0; j < hid_dim; ++j) res->hidden[j] = h[j];

    int used[SEQ_MAX_DIM] = {0};
    int top_k = (out_dim < 3) ? (int)out_dim : 3;
    for (int k = 0; k < 3; ++k) {
        res->top_idx[k] = 0;
        res->top_prob[k] = 0.0f;
    }
    for (int k = 0; k < top_k; ++k) {
        int best = -1;
        for (uint32_t o = 0; o < out_dim; ++o) {
            if (!used[o] && (best < 0 || res->probs[o] > res->probs[best])) {
                best = (int)o;
            }
        }
        if (best >= 0) {
            used[best] = 1;
            res->top_idx[k] = best;
            res->top_prob[k] = res->probs[best];
        }
    }
    return 0;
}

#endif /* FIPSOS_SIMD_SEQ_NN_H */
