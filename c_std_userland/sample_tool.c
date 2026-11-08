static long sys_write(int fd, const void *buf, unsigned long count) {
    long ret;
    __asm__ volatile("syscall"
                    : "=a"(ret)
                    : "a"(1), "D"(fd), "S"(buf), "d"(count)
                    : "rcx", "r11", "memory");
    return ret;
}

static void sys_exit(int code) {
    __asm__ volatile("syscall" :: "a"(60), "D"(code) : "rcx", "r11", "memory");
    while (1) {}
}

void _start(void) {
    const char msg[] =
        "\n======================================================\n"
        "  HOLA DESDE /disk/bin/sample_tool (FipSOs v2.3)\n"
        "  Descargado e instalado dinamicamente con 'pkg'!\n"
        "  Ejecutado inmediatamente sin reiniciar el sistema.\n"
        "======================================================\n\n";
    sys_write(1, msg, sizeof(msg) - 1);
    sys_exit(0);
}
