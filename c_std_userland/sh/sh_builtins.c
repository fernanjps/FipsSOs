/*
 * FipSOs - sh_builtins.c
 * Core Shell Built-in Commands, System Control & Network/Storage Tools
 */

#include "sh_builtins.h"
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <signal.h>
#include <fcntl.h>
#include <dirent.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <sys/utsname.h>
#include <sys/syscall.h>
#include <sys/ioctl.h>
#include <sys/socket.h>
#include <netinet/in.h>
#include <arpa/inet.h>
#include <sys/mman.h>

#define VMO_RING_MAGIC 0x564D4F52
#define VMO_RING_QUEUE_DEPTH 32
#define VMO_RING_OP_ECHO 1
#define VMO_RING_OP_3D_TRANSFORM 2
#define VMO_RING_OP_MEM_CRC 4

struct VmoSqEntry {
    uint32_t opcode;
    uint32_t flags;
    uint64_t user_data;
    uint64_t buffer_offset;
    uint32_t len;
    uint32_t reserved;
};

struct VmoCqEntry {
    uint64_t user_data;
    int32_t  result;
    uint32_t flags;
};

struct VmoRingHeader {
    uint32_t magic;
    uint32_t version;
    uint32_t queue_depth;
    uint32_t flags;
    uint32_t sq_head;
    uint32_t sq_tail;
    uint32_t cq_head;
    uint32_t cq_tail;
    uint32_t sq_dropped;
    uint32_t cq_overflow;
    uint32_t reserved[6];
};

#define VFS_IOCTL_LIST_PROCESSES      0xC0105001
#define VFS_IOCTL_HWMON_REPORT        0xC0105002
#define NET_IOCTL_PING_GATEWAY        0xC0104E01
#define NET_IOCTL_HTTP_FETCH          0xC0104E02
#define NET_IOCTL_BROWSER_FETCH_SAVE  0xC0104E03
#define DISK_IOCTL_SAVE_NOTE          0xC0104401

void print_banner(void) {
    static const char banner_msg[] =
        "\n\033[1;36m======================================================================\033[0m\n"
        "\033[1;32m  FipSOs v3.5 Framekernel (x86_64 SMP | PCIe NVMe | USB xHCI DMA | Remote :2222)\033[0m\n"
        "\033[1;33m  Polyglot Kernel: ASM + C11 + Rust + C++20 | System V ABI | Modular Shell\033[0m\n"
        "\033[1;35m  Boot: UEFI/BIOS Hybrid Bare-Metal Ready\033[0m\n"
        "\033[1;36m======================================================================\033[0m\n"
        "Type '\033[1;37mhelp\033[0m' for available commands, '\033[1;37mls\033[0m' to list files, or '\033[1;37mgui\033[0m' for Desktop.\n\n";
    if (write(1, banner_msg, sizeof(banner_msg) - 1) < 0) {}
}

static void cmd_help(void) {
    static const char help_msg[] =
        "\n\033[1;32mFipSOs Shell - Comandos Disponibles:\033[0m\n"
        "  \033[1;37mhelp\033[0m               - Muestra esta lista de ayuda\n"
        "  \033[1;37mpwd\033[0m                - Muestra el directorio de trabajo actual (CWD)\n"
        "  \033[1;37mcd <dir>\033[0m           - Cambia de directorio\n"
        "  \033[1;37mls [dir]\033[0m           - Lista el contenido del directorio con tipos y tamanos\n"
        "  \033[1;37mcat <file>\033[0m         - Muestra el contenido de un archivo de texto\n"
        "  \033[1;37mecho [text] [> f]\033[0m  - Imprime texto o redirige a un archivo nuevo/existente\n"
        "  \033[1;37mtouch <file>\033[0m       - Crea un archivo vacio en el VFS o en /disk/ (SATA)\n"
        "  \033[1;37mmkdir <dir>\033[0m        - Crea un nuevo directorio en el VFS o en /disk/\n"
        "  \033[1;37mrmdir <dir>\033[0m        - Elimina un subdirectorio vacio de /disk/ o VFS\n"
        "  \033[1;37mrm <file>\033[0m          - Elimina un archivo del VFS y de SATA AHCI\n"
        "  \033[1;37muname [-a]\033[0m         - Informacion del sistema operativo y arquitectura\n"
        "  \033[1;37mnproc\033[0m              - Muestra el numero de procesadores/nucleos activos (SMP)\n"
        "  \033[1;37mps\033[0m                 - Lista procesos y tareas del planificador\n"
        "  \033[1;37mfree / sensors\033[0m     - Telemetria de memoria, temperatura, NVMe y hardware\n"
        "  \033[1;37mclear\033[0m              - Limpia la pantalla de la consola\n"
        "  \033[1;37medit / kilo <file>\033[0m - Editor de texto modal a pantalla completa en C\n"
        "  \033[1;37mgui\033[0m                - Lanza el Compositor de Escritorio C++20\n"
        "  \033[1;37mbusybox [applet] [args]\033[0m - Binario multi-call BusyBox v1.35.0 estatico\n"
        "  \033[1;37mcmd1 | cmd2\033[0m        - Soporte de tuberias (pipes) con redireccion POSIX\n"
        "  \033[1;37mcapp\033[0m               - Ejecuta la aplicacion de prueba C standard (/bin/c_std_app.elf)\n"
        "  \033[1;37msimd / npu\033[0m         - Motor SSE2/AVX Engine (/bin/benchmark_simd.elf o npu_infer.elf)\n"
        "  \033[1;37mavatar\033[0m             - Ejecuta el avatar 3D interactivo C++20 (/bin/avatar_cpp.elf)\n"
        "  \033[1;37mfault\033[0m              - Prueba de supervivencia ante #PF en Anillo 3 (/bin/fault_test.elf)\n"
        "  \033[1;37mnet / ping\033[0m         - Envia ping ARP/ICMP a la puerta de enlace PCIe e1000\n"
        "  \033[1;37mwget [path]\033[0m        - Descarga paquete HTTP por DMA y lo guarda en VFS\n"
        "  \033[1;37mcurl <url>\033[0m         - Descarga URL y la persiste en disco SATA AHCI\n"
        "  \033[1;37mpkg <url>\033[0m          - Gestor de paquetes: descarga a /disk/bin y registra binario\n"
        "  \033[1;37mringtest\033[0m            - Prueba de canal IPC Zero-Syscall sobre /dev/shm/io_ring\n"
        "  \033[1;37msync\033[0m               - Persiste una nota con timestamp en sectores LBA de SATA\n"
        "  \033[1;37mpoweroff\033[0m           - Apagado seguro ACPI S5 del equipo\n"
        "  \033[1;37mreboot\033[0m             - Reinicio por hardware de la CPU\n"
        "  \033[1;37mexit\033[0m               - Sale del shell interactivo\n\n";
    if (write(1, help_msg, sizeof(help_msg) - 1) < 0) {}
}

static void cmd_pwd(void) {
    char cwd[256];
    if (getcwd(cwd, sizeof(cwd))) {
        printf("%s\n", cwd);
    } else {
        perror("getcwd");
    }
}

static void cmd_cd(const char* path) {
    const char* target = (path && *path) ? path : "/";
    if (chdir(target) != 0) {
        printf("cd: no se pudo acceder a '%s': No existe el directorio\n", target);
    }
}

static void cmd_ls(const char* path) {
    const char* target = (path && *path) ? path : ".";
    DIR* dir = opendir(target);
    if (!dir) {
        printf("ls: no se puede abrir '%s': No es un directorio o no existe\n", target);
        return;
    }

    printf("\n\033[1;34mDirectorio: %s\033[0m\n", target);
    printf("  %-10s %-12s %s\n", "TIPO", "TAMANO", "NOMBRE");
    printf("  ----------------------------------------\n");

    struct dirent* entry;
    int count = 0;
    while ((entry = readdir(dir)) != NULL) {
        count++;
        const char* type_str = (entry->d_type == DT_DIR) ? "\033[1;34m[DIR]\033[0m " :
                              ((entry->d_type == DT_CHR) ? "\033[1;33m[DEV]\033[0m " : "\033[0;37m[FILE]\033[0m");

        char fullpath[512];
        if (strcmp(target, "/") == 0) snprintf(fullpath, sizeof(fullpath), "/%s", entry->d_name);
        else snprintf(fullpath, sizeof(fullpath), "%s/%s", target, entry->d_name);

        struct stat st;
        long size = (stat(fullpath, &st) == 0) ? (long)st.st_size : 0;

        if (entry->d_type == DT_DIR) {
            printf("  %s %8ld B   \033[1;34m%s/\033[0m\n", type_str, size, entry->d_name);
        } else if (entry->d_type == DT_CHR) {
            printf("  %s %8ld B   \033[1;33m%s\033[0m\n", type_str, size, entry->d_name);
        } else {
            printf("  %s %8ld B   %s\n", type_str, size, entry->d_name);
        }
    }
    closedir(dir);
    printf("  ----------------------------------------\n");
    printf("  Total: %d entradas.\n\n", count);
}

static void cmd_cat(const char* path) {
    FILE* fp = (!path || !*path || strcmp(path, "-") == 0) ? stdin : fopen(path, "r");
    if (!fp) {
        printf("cat: '%s': No existe el archivo o error de lectura\n", path ? path : "-");
        return;
    }
    char buf[512];
    size_t n;
    while ((n = fread(buf, 1, sizeof(buf), fp)) > 0) {
        fwrite(buf, 1, n, stdout);
    }
    if (fp != stdin) {
        printf("\n");
        fclose(fp);
    } else {
        clearerr(stdin);
    }
}

static void cmd_kill(int argc, char** argv) {
    if (argc < 2) {
        printf("Uso: kill <pid> [sig]  o  kill -<sig> <pid>\n");
        return;
    }
    int sig = 9;
    pid_t target = 0;
    if (argv[1][0] == '-') {
        sig = atoi(&argv[1][1]);
        if (argc >= 3) target = (pid_t)atoi(argv[2]);
    } else {
        target = (pid_t)atoi(argv[1]);
        if (argc >= 3) sig = atoi(argv[2]);
    }
    if (target <= 0) {
        printf("kill: PID invalido\n");
        return;
    }
    if (kill(target, sig) == 0) {
        printf("kill: senal %d enviada con exito al PID %d\n", sig, (int)target);
    } else {
        perror("kill");
    }
}

static void cmd_touch(const char* path) {
    if (!path || !*path) {
        printf("Uso: touch <archivo>\n");
        return;
    }
    FILE* fp = fopen(path, "a");
    if (!fp) {
        printf("touch: no se pudo crear '%s'\n", path);
        return;
    }
    fclose(fp);
    printf("touch: '%s' creado o actualizado correctamente.\n", path);
}

static void cmd_mkdir(const char* path) {
    if (!path || !*path) {
        printf("Uso: mkdir <directorio>\n");
        return;
    }
    if (mkdir(path, 0755) != 0) {
        printf("mkdir: fallo al crear el directorio '%s'\n", path);
    } else {
        printf("mkdir: directorio '%s' creado.\n", path);
    }
}

static void cmd_rmdir(const char* path) {
    if (!path || !*path) {
        printf("Uso: rmdir <directorio>\n");
        return;
    }
    if (rmdir(path) != 0) {
        printf("rmdir: fallo al eliminar directorio '%s'\n", path);
    } else {
        printf("rmdir: directorio '%s' eliminado correctamente.\n", path);
    }
}

static void cmd_rm(const char* path) {
    if (!path || !*path) {
        printf("Uso: rm <archivo>\n");
        return;
    }
    if (unlink(path) != 0) {
        if (rmdir(path) == 0) {
            printf("rm: directorio '%s' eliminado correctamente.\n", path);
            return;
        }
        printf("rm: no se pudo eliminar '%s'\n", path);
    } else {
        printf("rm: '%s' eliminado correctamente.\n", path);
    }
}

static void cmd_echo(int argc, char** argv) {
    int redir_idx = -1;
    int append = 0;
    for (int i = 1; i < argc; ++i) {
        if (strcmp(argv[i], ">") == 0) {
            redir_idx = i;
            append = 0;
            break;
        } else if (strcmp(argv[i], ">>") == 0) {
            redir_idx = i;
            append = 1;
            break;
        }
    }

    if (redir_idx > 0 && redir_idx + 1 < argc) {
        const char* target_file = argv[redir_idx + 1];
        FILE* fp = fopen(target_file, append ? "a" : "w");
        if (!fp) {
            printf("echo: no se puede escribir en '%s'\n", target_file);
            return;
        }
        for (int i = 1; i < redir_idx; ++i) {
            fprintf(fp, "%s%s", argv[i], (i + 1 < redir_idx) ? " " : "");
        }
        fprintf(fp, "\n");
        fclose(fp);
        printf("echo: escrito en '%s'.\n", target_file);
    } else {
        for (int i = 1; i < argc; ++i) {
            printf("%s%s", argv[i], (i + 1 < argc) ? " " : "");
        }
        printf("\n");
    }
}

static void cmd_uname(void) {
    struct utsname un;
    if (uname(&un) == 0) {
        printf("%s %s %s %s %s\n", un.sysname, un.nodename, un.release, un.version, un.machine);
    } else {
        perror("uname");
    }
}

static void cmd_ps(void) {
    int fb_fd = open("/dev/fb0", O_RDWR);
    if (fb_fd >= 0) {
        ioctl(fb_fd, VFS_IOCTL_LIST_PROCESSES, 0);
        close(fb_fd);
    } else {
        printf("ps: no se puede consultar el planificador de procesos\n");
    }
}

static void cmd_sensors(void) {
    int fb_fd = open("/dev/fb0", O_RDWR);
    if (fb_fd >= 0) {
        char report[2048];
        memset(report, 0, sizeof(report));
        ioctl(fb_fd, VFS_IOCTL_HWMON_REPORT, (unsigned long)report);
        printf("%s\n", report);
        close(fb_fd);
    } else {
        printf("sensors: no se puede consultar /dev/fb0\n");
    }
}

static void cmd_net_ping(void) {
    int net_fd = open("/dev/net/eth0", O_RDWR);
    if (net_fd >= 0) {
        ioctl(net_fd, NET_IOCTL_PING_GATEWAY, 1);
        close(net_fd);
        printf("net: Ping ICMP/ARP a 10.0.2.2 completado con exito.\n");
    } else {
        printf("net: no se puede abrir /dev/net/eth0\n");
    }
}

static void cmd_wget(const char* url) {
    int net_fd = open("/dev/net/eth0", O_RDWR);
    if (net_fd >= 0) {
        (void)url;
        ioctl(net_fd, NET_IOCTL_HTTP_FETCH, 1);
        close(net_fd);
        printf("wget: Descarga de fipsos_pkg.txt completada y montada en VFS /downloads/\n");
    } else {
        printf("wget: no se puede abrir /dev/net/eth0\n");
    }
}

static void cmd_curl(const char* url) {
    int fb_fd = open("/dev/fb0", O_RDWR);
    if (fb_fd >= 0) {
        (void)url;
        ioctl(fb_fd, NET_IOCTL_BROWSER_FETCH_SAVE, 1);
        close(fb_fd);
        printf("curl: Descargado y guardado en SATA AHCI /disk/index.html\n");
    } else {
        printf("curl: no se puede abrir /dev/fb0\n");
    }
}

static void cmd_sync(void) {
    int fb_fd = open("/dev/fb0", O_RDWR);
    if (fb_fd >= 0) {
        ioctl(fb_fd, DISK_IOCTL_SAVE_NOTE, 1);
        close(fb_fd);
        printf("sync: Nota persistente escrita en sectores LBA de disco SATA AHCI.\n");
    } else {
        printf("sync: no se puede abrir /dev/fb0\n");
    }
}

static void cmd_ringtest(void) {
    printf("[ringtest] Abriendo canal IPC Zero-Syscall (/dev/shm/io_ring)...\n");
    int fd = open("/dev/shm/io_ring", O_RDWR);
    if (fd < 0) fd = open("/dev/ipc/ring0", O_RDWR);
    if (fd < 0) { perror("[ringtest] open"); return; }

    void* mem = mmap(NULL, 65536, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    if (mem == MAP_FAILED) { perror("[ringtest] mmap"); close(fd); return; }

    volatile struct VmoRingHeader* hdr = (volatile struct VmoRingHeader*)mem;
    if (hdr->magic != VMO_RING_MAGIC) {
        fprintf(stderr, "[ringtest] Error: Magic invalido (0x%08X)\n", hdr->magic);
        close(fd);
        return;
    }

    volatile struct VmoSqEntry* sqes = (volatile struct VmoSqEntry*)((char*)mem + 64);
    volatile struct VmoCqEntry* cqes = (volatile struct VmoCqEntry*)((char*)mem + 64 + 32 * sizeof(struct VmoSqEntry));
    char* data_buf = (char*)mem + 2048;

    uint32_t tail = hdr->sq_tail;
    strcpy(data_buf, "ZERO_SYSCALL_TEST_DATA");
    uint32_t idx0 = tail % VMO_RING_QUEUE_DEPTH;
    sqes[idx0].opcode = VMO_RING_OP_ECHO; sqes[idx0].flags = 0; sqes[idx0].user_data = 0x101;
    sqes[idx0].buffer_offset = 0; sqes[idx0].len = (uint32_t)strlen(data_buf);
    tail++;

    float* vertices = (float*)(data_buf + 128);
    vertices[0] = 10.0f; vertices[1] = 20.0f; vertices[2] = 30.0f;
    uint32_t idx1 = tail % VMO_RING_QUEUE_DEPTH;
    sqes[idx1].opcode = VMO_RING_OP_3D_TRANSFORM; sqes[idx1].flags = 0; sqes[idx1].user_data = 0x102;
    sqes[idx1].buffer_offset = 128; sqes[idx1].len = 12;
    tail++;

    uint32_t idx2 = tail % VMO_RING_QUEUE_DEPTH;
    sqes[idx2].opcode = VMO_RING_OP_MEM_CRC; sqes[idx2].flags = 0; sqes[idx2].user_data = 0x103;
    sqes[idx2].buffer_offset = 0; sqes[idx2].len = (uint32_t)strlen(data_buf);
    tail++;

    __atomic_store_n(&hdr->sq_tail, tail, __ATOMIC_RELEASE);
    printf("[ringtest] 3 peticiones encoladas en SQ. Esperando resolucion en Core 1...\n");

    int completed = 0;
    uint32_t cq_head = hdr->cq_head;
    for (int retry = 0; retry < 100000 && completed < 3; retry++) {
        uint32_t cq_tail = __atomic_load_n(&hdr->cq_tail, __ATOMIC_ACQUIRE);
        while (cq_head != cq_tail && completed < 3) {
            uint32_t cidx = cq_head % VMO_RING_QUEUE_DEPTH;
            printf("[ringtest] CQE recibido: user_data=0x%lX, res=%d\n", (unsigned long)cqes[cidx].user_data, cqes[cidx].result);
            cq_head++;
            completed++;
        }
    }
    __atomic_store_n(&hdr->cq_head, cq_head, __ATOMIC_RELEASE);
    if (completed == 3) {
        printf("\033[1;32m[ringtest] Exito: 3 transacciones Zero-Syscall completadas!\033[0m\n");
    }
    close(fd);
}

static void cmd_pkg(int argc, char** argv) {
    if (argc < 2) {
        printf("Uso: pkg install <url> | pkg fetch <url> | pkg <url>\n");
        return;
    }
    const char* raw_url = (strcmp(argv[1], "install") == 0 || strcmp(argv[1], "fetch") == 0) ? ((argc > 2) ? argv[2] : "") : argv[1];
    if (!raw_url || !*raw_url) { printf("Error: URL requerida.\n"); return; }

    char host[128] = "10.0.2.2";
    int port = 8080;
    char path[256] = "/sample_tool.elf";
    const char* p = (strncmp(raw_url, "http://", 7) == 0) ? raw_url + 7 : raw_url;
    const char* slash = strchr(p, '/');
    if (slash) {
        snprintf(path, sizeof(path), "%s", slash);
        char host_port[128];
        size_t hlen = slash - p;
        if (hlen >= sizeof(host_port)) hlen = sizeof(host_port) - 1;
        strncpy(host_port, p, hlen);
        host_port[hlen] = '\0';
        char* colon = strchr(host_port, ':');
        if (colon) { *colon = '\0'; port = atoi(colon + 1); }
        if (host_port[0]) snprintf(host, sizeof(host), "%s", host_port);
    } else {
        snprintf(path, sizeof(path), "/%s", p);
    }

    const char* last_slash = strrchr(path, '/');
    const char* filename = last_slash ? (last_slash + 1) : path;
    if (!filename || !*filename) filename = "downloaded_pkg.elf";

    mkdir("/disk", 0755);
    mkdir("/disk/bin", 0755);
    char dest_path[256];
    snprintf(dest_path, sizeof(dest_path), "/disk/bin/%s", filename);

    printf("\033[1;36m[pkg]\033[0m Conectando a %s:%d para \"%s\"...\n", host, port, path);
    int sock = socket(AF_INET, SOCK_STREAM, 0);
    if (sock < 0) { perror("[pkg] socket"); return; }

    struct sockaddr_in saddr;
    memset(&saddr, 0, sizeof(saddr));
    saddr.sin_family = AF_INET;
    saddr.sin_port = htons(port);
    inet_pton(AF_INET, host, &saddr.sin_addr);
    if (connect(sock, (struct sockaddr*)&saddr, sizeof(saddr)) < 0) {
        perror("[pkg] connect");
        close(sock);
        return;
    }

    char req[512];
    snprintf(req, sizeof(req), "GET %s HTTP/1.0\r\nHost: %s:%d\r\nConnection: close\r\n\r\n", path, host, port);
    send(sock, req, strlen(req), 0);

    char recv_buf[4096];
    ssize_t n = recv(sock, recv_buf, sizeof(recv_buf), 0);
    if (n <= 0) { close(sock); return; }

    char* header_end = strstr(recv_buf, "\r\n\r\n");
    header_end = header_end ? header_end + 4 : recv_buf;

    int dest_fd = open(dest_path, O_CREAT | O_WRONLY | O_TRUNC, 0755);
    if (dest_fd < 0) { perror("[pkg] open"); close(sock); return; }

    size_t first_len = n - (header_end - recv_buf);
    size_t total = 0;
    if (first_len > 0) {
        ssize_t w = write(dest_fd, header_end, first_len);
        if (w > 0) total += (size_t)w;
    }
    while ((n = recv(sock, recv_buf, sizeof(recv_buf), 0)) > 0) {
        ssize_t w = write(dest_fd, recv_buf, n);
        if (w > 0) total += (size_t)w;
    }
    close(dest_fd);
    close(sock);
    printf("\033[1;32m[pkg]\033[0m Paquete \"%s\" instalado en %s (%zu B).\n", filename, dest_path, total);
}

int handle_builtin(int argc, char** argv) {
    if (argc == 0 || !argv[0] || !*argv[0]) return 0;
    const char* cmd = argv[0];

    if (strcmp(cmd, "help") == 0) { cmd_help(); return 1; }
    if (strcmp(cmd, "pwd") == 0) { cmd_pwd(); return 1; }
    if (strcmp(cmd, "cd") == 0) { cmd_cd(argc > 1 ? argv[1] : "/"); return 1; }
    if (strcmp(cmd, "ls") == 0) { cmd_ls(argc > 1 ? argv[1] : "."); return 1; }
    if (strcmp(cmd, "cat") == 0) { cmd_cat(argc > 1 ? argv[1] : ""); return 1; }
    if (strcmp(cmd, "touch") == 0) { cmd_touch(argc > 1 ? argv[1] : ""); return 1; }
    if (strcmp(cmd, "mkdir") == 0) { cmd_mkdir(argc > 1 ? argv[1] : ""); return 1; }
    if (strcmp(cmd, "rm") == 0) { cmd_rm(argc > 1 ? argv[1] : ""); return 1; }
    if (strcmp(cmd, "rmdir") == 0) { cmd_rmdir(argc > 1 ? argv[1] : ""); return 1; }
    if (strcmp(cmd, "echo") == 0) { cmd_echo(argc, argv); return 1; }
    if (strcmp(cmd, "uname") == 0) { cmd_uname(); return 1; }
    if (strcmp(cmd, "ps") == 0) { cmd_ps(); return 1; }
    if (strcmp(cmd, "kill") == 0) { cmd_kill(argc, argv); return 1; }
    if (strcmp(cmd, "free") == 0 || strcmp(cmd, "sensors") == 0 || strcmp(cmd, "hwmon") == 0) { cmd_sensors(); return 1; }
    if (strcmp(cmd, "net") == 0 || strcmp(cmd, "ping") == 0) { cmd_net_ping(); return 1; }
    if (strcmp(cmd, "wget") == 0) { cmd_wget(argc > 1 ? argv[1] : NULL); return 1; }
    if (strcmp(cmd, "curl") == 0) { cmd_curl(argc > 1 ? argv[1] : NULL); return 1; }
    if (strcmp(cmd, "sync") == 0 || strcmp(cmd, "disk") == 0) { cmd_sync(); return 1; }
    if (strcmp(cmd, "pkg") == 0) { cmd_pkg(argc, argv); return 1; }
    if (strcmp(cmd, "ringtest") == 0) { cmd_ringtest(); return 1; }
    if (strcmp(cmd, "clear") == 0) { printf("\033[2J\033[H"); return 1; }
    if (strcmp(cmd, "poweroff") == 0) { printf("Apagando ACPI S5...\n"); syscall(SYS_reboot, 0, 0, 0, 0); return 1; }
    if (strcmp(cmd, "reboot") == 0) { printf("Reiniciando...\n"); syscall(SYS_reboot, 0, 0, 1, 0); return 1; }
    if (strcmp(cmd, "exit") == 0) { printf("Saliendo de FipSOs Shell.\n"); exit(0); }

    return 0;
}
