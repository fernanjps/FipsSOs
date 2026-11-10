#include <stdint.h>
#include <stddef.h>

/*
 * FipSOs Capa 0 C11 Architectural Exception & Panic Decoder.
 * Provides freestanding POSIX signal translation and formatted
 * diagnostic records for all 32 x86_64 CPU fault vectors (0..31).
 */

typedef struct {
    uint64_t    vector;
    const char* mnemonic;
    const char* name;
    int32_t     posix_signal;
} c_exception_entry_t;

static const c_exception_entry_t EXCEPTION_TABLE[32] = {
    {0,  "#DE",  "Divide-by-Zero Error",                -8},  /* SIGFPE */
    {1,  "#DB",  "Debug / Single Step",                 -5},  /* SIGTRAP */
    {2,  "NMI",  "Non-Maskable Hardware Interrupt",     -11}, /* SIGSEGV */
    {3,  "#BP",  "Breakpoint (INT3)",                   -5},  /* SIGTRAP */
    {4,  "#OF",  "Overflow (INTO)",                     -11}, /* SIGSEGV */
    {5,  "#BR",  "BOUND Range Exceeded",                -11}, /* SIGSEGV */
    {6,  "#UD",  "Invalid Opcode (Undefined Instruction)", -4}, /* SIGILL */
    {7,  "#NM",  "Device Not Available (No FPU)",       -8},  /* SIGFPE */
    {8,  "#DF",  "Double Fault (Stack/Exception Error)", -11}, /* SIGSEGV */
    {9,  "#CSO", "Coprocessor Segment Overrun",        -11}, /* SIGSEGV */
    {10, "#TS",  "Invalid TSS",                         -11}, /* SIGSEGV */
    {11, "#NP",  "Segment Not Present",                 -7},  /* SIGBUS */
    {12, "#SS",  "Stack-Segment Fault",                 -11}, /* SIGSEGV */
    {13, "#GP",  "General Protection Fault",            -13}, /* SIGSEGV */
    {14, "#PF",  "Page Fault",                          -11}, /* SIGSEGV */
    {15, "#RES", "Reserved Exception 15",               -11}, /* SIGSEGV */
    {16, "#MF",  "x87 FPU Floating-Point Error",        -8},  /* SIGFPE */
    {17, "#AC",  "Alignment Check",                     -7},  /* SIGBUS */
    {18, "#MC",  "Machine Check",                       -11}, /* SIGSEGV */
    {19, "#XM",  "SIMD Floating-Point Exception",       -8},  /* SIGFPE */
    {20, "#VE",  "Virtualization Exception",            -11}, /* SIGSEGV */
    {21, "#CP",  "Control Protection Exception",        -11}, /* SIGSEGV */
    {22, "#RES22","Reserved Exception 22",              -11}, /* SIGSEGV */
    {23, "#RES23","Reserved Exception 23",              -11}, /* SIGSEGV */
    {24, "#RES24","Reserved Exception 24",              -11}, /* SIGSEGV */
    {25, "#RES25","Reserved Exception 25",              -11}, /* SIGSEGV */
    {26, "#RES26","Reserved Exception 26",              -11}, /* SIGSEGV */
    {27, "#RES27","Reserved Exception 27",              -11}, /* SIGSEGV */
    {28, "#HV",  "Hypervisor Injection Exception",      -11}, /* SIGSEGV */
    {29, "#VC",  "VMM Communication Exception",         -11}, /* SIGSEGV */
    {30, "#SX",  "Security Exception",                  -11}, /* SIGSEGV */
    {31, "#RES31","Reserved Exception 31",              -11}, /* SIGSEGV */
};

const char* c_lookup_exception_mnemonic(uint64_t vector) {
    if (vector < 32) {
        return EXCEPTION_TABLE[vector].mnemonic;
    }
    return "#UNK";
}

const char* c_lookup_exception_name(uint64_t vector) {
    if (vector < 32) {
        return EXCEPTION_TABLE[vector].name;
    }
    return "Unknown Exception";
}

int32_t c_lookup_posix_signal(uint64_t vector) {
    if (vector < 32) {
        return EXCEPTION_TABLE[vector].posix_signal;
    }
    return -11; /* SIGSEGV fallback */
}

static size_t fmt_hex_u64(char* buf, size_t pos, size_t max_len, uint64_t val) {
    static const char hex_digits[] = "0123456789abcdef";
    if (pos + 18 >= max_len) {
        return pos;
    }
    buf[pos++] = '0';
    buf[pos++] = 'x';
    int started = 0;
    for (int shift = 60; shift >= 0; shift -= 4) {
        uint8_t nibble = (uint8_t)((val >> shift) & 0x0FU);
        if (nibble != 0 || started || shift == 0) {
            started = 1;
            buf[pos++] = hex_digits[nibble];
        }
    }
    return pos;
}

static size_t fmt_str(char* buf, size_t pos, size_t max_len, const char* str) {
    if (!str) return pos;
    while (*str && pos + 1 < max_len) {
        buf[pos++] = *str++;
    }
    return pos;
}

size_t c_format_panic_diagnostic(
    char* buf,
    size_t max_len,
    uint64_t vector,
    uint64_t error_code,
    uint64_t cr2,
    uint64_t rip,
    uint64_t cs,
    uint64_t rflags,
    uint64_t rsp
) {
    if (buf == NULL || max_len < 32) {
        return 0;
    }

    size_t p = 0;
    p = fmt_str(buf, p, max_len, "[KERNEL PANIC: ");
    p = fmt_str(buf, p, max_len, c_lookup_exception_mnemonic(vector));
    p = fmt_str(buf, p, max_len, " ");
    p = fmt_str(buf, p, max_len, c_lookup_exception_name(vector));
    p = fmt_str(buf, p, max_len, "]\n  RIP=");
    p = fmt_hex_u64(buf, p, max_len, rip);
    p = fmt_str(buf, p, max_len, " | CS=");
    p = fmt_hex_u64(buf, p, max_len, cs);
    p = fmt_str(buf, p, max_len, " | RSP=");
    p = fmt_hex_u64(buf, p, max_len, rsp);
    p = fmt_str(buf, p, max_len, "\n  RFLAGS=");
    p = fmt_hex_u64(buf, p, max_len, rflags);
    p = fmt_str(buf, p, max_len, " | ERR=");
    p = fmt_hex_u64(buf, p, max_len, error_code);
    p = fmt_str(buf, p, max_len, " | CR2=");
    p = fmt_hex_u64(buf, p, max_len, cr2);

    /* Detail decoding for Page Faults (#PF) */
    if (vector == 14) {
        p = fmt_str(buf, p, max_len, "\n  PF Flags: ");
        p = fmt_str(buf, p, max_len, (error_code & 1) ? "[PRESENT] " : "[NOT-PRESENT] ");
        p = fmt_str(buf, p, max_len, (error_code & 2) ? "[WRITE] " : "[READ] ");
        p = fmt_str(buf, p, max_len, (error_code & 4) ? "[USER] " : "[SUPERVISOR] ");
        if (error_code & 16) {
            p = fmt_str(buf, p, max_len, "[INSTRUCTION-FETCH] ");
        }
    }

    p = fmt_str(buf, p, max_len, "\n");

    if (p < max_len) {
        buf[p] = '\0';
    } else {
        buf[max_len - 1] = '\0';
    }
    return p;
}
