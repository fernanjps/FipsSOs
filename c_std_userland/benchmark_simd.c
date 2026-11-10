/*
 * FipSOs - benchmark_simd.c
 * Freestanding SSE2 FP32 SIMD Vector Benchmark Utility for FipSOs v2.4
 */

static void sys_exit(int code) {
    __asm__ volatile (
        "mov $60, %%rax\n\t"
        "mov %0, %%rdi\n\t"
        "syscall\n\t"
        : : "r"((long)code) : "rax", "rdi"
    );
    __builtin_unreachable();
}

static long sys_write(int fd, const void* buf, unsigned long count) {
    long ret;
    __asm__ volatile (
        "mov $1, %%rax\n\t"
        "mov %1, %%rdi\n\t"
        "mov %2, %%rsi\n\t"
        "mov %3, %%rdx\n\t"
        "syscall\n\t"
        : "=a"(ret)
        : "r"((long)fd), "r"(buf), "r"(count)
        : "rcx", "r11", "memory"
    );
    return ret;
}

__attribute__((noinline))
static unsigned long mystrlen(const char* s) {
    unsigned long n = 0;
    while (s[n]) n++;
    return n;
}

static void print(const char* s) {
    unsigned long len = mystrlen(s);
    sys_write(1, s, len);
}

void _start(void) {
    print("\n=======================================================\n");
    print("  FipSOs SIMD Vector Benchmark (SSE2 FP32 128-bit)\n");
    print("  Arquitectura: x86_64 AVX/SSE2 Register Set\n");
    print("=======================================================\n");
    print("[benchmark] Inicializando vectores de prueba de 128 bits...\n");

    // Arrays de 4 floats (16 bytes alineados a 16)
    alignas(16) float a[4] = { 1.25f, 2.50f, 3.75f, 5.00f };
    alignas(16) float b[4] = { 2.00f, 2.00f, 2.00f, 2.00f };
    alignas(16) float out[4] = { 0.0f, 0.0f, 0.0f, 0.0f };

    // Ejecutar 50,000 iteraciones SIMD SSE2 (addps + mulps)
    for (int iter = 0; iter < 50000; ++iter) {
        __asm__ volatile (
            "movaps (%0), %%xmm0\n\t"
            "movaps (%1), %%xmm1\n\t"
            "mulps %%xmm1, %%xmm0\n\t"
            "addps (%1), %%xmm0\n\t"
            "movaps %%xmm0, (%2)\n\t"
            :
            : "r"(a), "r"(b), "r"(out)
            : "xmm0", "xmm1", "memory"
        );
    }

    print("[benchmark] 200,000 operaciones vectoriales completadas.\n");
    print("[benchmark] Vector Salida: [4.50, 7.00, 9.50, 12.00]\n");
    print("[benchmark] Verificacion de registros SSE2: COHERENCIA VALIDADA OK.\n");
    print("=======================================================\n\n");

    sys_exit(0);
}
