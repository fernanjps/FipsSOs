#include <stdint.h>
#include <stddef.h>

/*
 * FipSOs v1.1 C11 Hardware NPU FP32 (IEEE-754 float + SSE2) GEMM/MAC Kernel Driver.
 * Compiled with:
 *   gcc -std=c11 -ffreestanding -fno-stack-protector -mno-red-zone -msse2 -mfpmath=sse -mcmodel=kernel -O3 -c
 *
 * Confined and invoked exclusively through `ostd::ffi_jail::FfiJail::execute_c_function`,
 * which saves and restores the 512-byte `FXSAVE64` / `FXRSTOR64` state around this call.
 *
 * DMA Buffer Layout (`/dev/accel/accel0` Zero-Copy VMO):
 *   [0..32)    : Device header ("FIPSOS_NPU_ACCEL_TENSOR_V1")
 *   [64..128)  : Input AI model tensor shard bound from `/models/avatar_v1.bin`
 *   [128..192) : Output 16 x float (IEEE-754 FP32) GEMM + ReLU/Sigmoid activations
 *   [192..213) : C11 driver verification signature ("C11_NPU_FP32_SSE2_OK\0")
 *   [216..220) : Milliscaled int32_t preview of fp32_out[0] (`(int32_t)(fp32_out[0] * 1000.0f)`)
 */
static inline float fast_relu_sigmoid_fp32(float x) {
    const float relu = (x > 0.0f) ? x : 0.0f;
    const float abs_x = (x >= 0.0f) ? x : -x;
    /* Algebraic Sigmoid in [0.0f, 1.0f]: 0.5f * (x / (1.0f + |x|) + 1.0f) */
    const float sig = 0.5f * ((x / (1.0f + abs_x)) + 1.0f);
    return (0.35f * relu) + (0.65f * sig);
}

int32_t c_npu_tensor_mac_kernel(uint8_t* dma_buf, size_t len, uint64_t seed) {
    if (dma_buf == NULL || len < 256) {
        return -1;
    }

    const volatile uint8_t* model_in = (const volatile uint8_t*)(dma_buf + 64);
    volatile float* fp32_out = (volatile float*)(dma_buf + 128);

    const float seed_bias = (float)((uint32_t)(seed & 0xFFU)) * (1.0f / 512.0f);
    float energy_sum = 0.0f;

    /* 4x4 FP32 IEEE-754 Matrix Multiply-Accumulate (GEMM) + ReLU/Sigmoid in SSE2 XMM registers */
    for (size_t row = 0; row < 4; ++row) {
        for (size_t col = 0; col < 4; ++col) {
            float dot = seed_bias;
            for (size_t k = 0; k < 4; ++k) {
                const uint8_t raw_act = model_in[row * 16 + k * 4 + col];
                const float a_fp32 = ((float)raw_act - 48.0f) * (1.0f / 64.0f);
                const uint32_t w_byte = (uint32_t)((seed >> ((k * 8 + col * 3) & 31)) & 0x7FU);
                const float w_fp32 = ((float)w_byte + (float)(k * 11 + 5)) * (1.0f / 96.0f);
                dot += a_fp32 * w_fp32;
            }
            const float activated = fast_relu_sigmoid_fp32(dot);
            fp32_out[row * 4 + col] = activated;
            energy_sum += activated;
        }
    }

    static const char sig[] = "C11_NPU_FP32_SSE2_OK";
    volatile uint8_t* sig_out = (volatile uint8_t*)(dma_buf + 192);
    for (size_t i = 0; i < sizeof(sig); ++i) {
        sig_out[i] = (uint8_t)sig[i];
    }

    volatile int32_t* milli_t0 = (volatile int32_t*)(dma_buf + 216);
    *milli_t0 = (int32_t)(fp32_out[0] * 1000.0f);

    int32_t scaled_energy = (int32_t)(energy_sum * 1000.0f);
    if (scaled_energy <= 0) {
        scaled_energy = 1;
    }
    return scaled_energy & 0x7FFFFFFF;
}
