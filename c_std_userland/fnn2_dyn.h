#ifndef FIPSOS_FNN2_DYN_H
#define FIPSOS_FNN2_DYN_H
#include "simd_nn_rt.h"

#define FNN2_MAGIC    0x324E4E46U /* "FNN2" */
#define FNN2_MAX_IN   16
#define FNN2_MAX_HID  32
#define FNN2_MAX_OUT  8

typedef struct {
    uint32_t magic;
    uint32_t in_dim;
    uint32_t hid_dim;
    uint32_t out_dim;
} Fnn2Header;

typedef struct {
    Fnn2Header hdr;
    float w1[FNN2_MAX_HID * FNN2_MAX_IN];
    float b1[FNN2_MAX_HID];
    float w2[FNN2_MAX_OUT * FNN2_MAX_HID];
    float b2[FNN2_MAX_OUT];
    float probs[FNN2_MAX_OUT];
    int64_t last_us;
} Fnn2Model;

static int fnn2_read_exact(int fd, void* dst, size_t n) {
    uint8_t* p = (uint8_t*)dst;
    size_t got = 0;
    while (got < n) {
        ssize_t r = read(fd, p + got, n - got);
        if (r <= 0) return 0;
        got += (size_t)r;
    }
    return 1;
}

static inline int fnn2_load(const char* path, Fnn2Model* m) {
    int fd = open(path, O_RDONLY);
    if (fd < 0) return 0;
    if (!fnn2_read_exact(fd, &m->hdr, sizeof(m->hdr)) || m->hdr.magic != FNN2_MAGIC) {
        close(fd); return 0;
    }
    if (m->hdr.in_dim == 0 || m->hdr.in_dim > FNN2_MAX_IN ||
        m->hdr.hid_dim == 0 || m->hdr.hid_dim > FNN2_MAX_HID ||
        m->hdr.out_dim == 0 || m->hdr.out_dim > FNN2_MAX_OUT) {
        close(fd); return 0;
    }
    int ok = fnn2_read_exact(fd, m->w1, sizeof(float) * m->hdr.hid_dim * m->hdr.in_dim)
          && fnn2_read_exact(fd, m->b1, sizeof(float) * m->hdr.hid_dim)
          && fnn2_read_exact(fd, m->w2, sizeof(float) * m->hdr.out_dim * m->hdr.hid_dim)
          && fnn2_read_exact(fd, m->b2, sizeof(float) * m->hdr.out_dim);
    close(fd);
    return ok;
}

static inline int fnn2_infer(Fnn2Model* m, const float* inputs, int n_in) {
    struct timespec t0, t1;
    clock_gettime(CLOCK_MONOTONIC, &t0);
    float in[FNN2_MAX_IN];
    for (uint32_t i = 0; i < m->hdr.in_dim; ++i) {
        in[i] = ((int)i < n_in) ? inputs[i] : 0.0f;
    }
    float hid[FNN2_MAX_HID], logits[FNN2_MAX_OUT];
    fnn_dense_fp32(in, m->w1, m->b1, hid, (int)m->hdr.in_dim, (int)m->hdr.hid_dim);
    fnn_relu_sse2(hid, (int)m->hdr.hid_dim);
    fnn_dense_fp32(hid, m->w2, m->b2, logits, (int)m->hdr.hid_dim, (int)m->hdr.out_dim);
    fnn_softmax(logits, m->probs, (int)m->hdr.out_dim);
    clock_gettime(CLOCK_MONOTONIC, &t1);
    int64_t dt_us = ((t1.tv_sec - t0.tv_sec) * 1000000LL) + ((t1.tv_nsec - t0.tv_nsec) / 1000LL);
    m->last_us = (dt_us > 0) ? dt_us : 1;
    int best = 0;
    for (uint32_t i = 1; i < m->hdr.out_dim; ++i) {
        if (m->probs[i] > m->probs[best]) best = (int)i;
    }
    return best;
}

#endif
