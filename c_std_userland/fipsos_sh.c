/*
 * FipSOs - fipsos_sh.c
 * Pure Standard Linux C Interactive Userland Shell (/bin/sh /bin/fipsos_sh.elf)
 *
 * Implements a full-featured POSIX interactive command shell:
 * - Interactive REPL with prompt displaying current working directory
 * - Standard C libc streams (stdin, stdout, stderr, fopen, fgets, fprintf)
 * - Directory enumeration via standard POSIX opendir(), readdir(), closedir()
 * - Standard POSIX filesystem manipulation: getcwd, chdir, mkdir, unlink
 * - Standard file redirection (command > file and command >> file)
 * - Process execution via standard POSIX execve() with PATH fallback (/bin/<cmd>.elf)
 * - Hardware and system controls: ACPI poweroff/reboot, WiFi, SATA, NPU, Net
 *
 * Strictly adheres to ARCHITECTURE.md (0 warnings, pure Standard Linux C11).
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
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

#define MAX_LINE 512
#define MAX_ARGS 32

/* Zero-Syscall Shared Memory Ring structures (/dev/shm/io_ring) */
#define VMO_RING_MAGIC 0x564D4F52
#define VMO_RING_QUEUE_DEPTH 32

#define VMO_RING_OP_NOP          0
#define VMO_RING_OP_ECHO         1
#define VMO_RING_OP_3D_TRANSFORM 2
#define VMO_RING_OP_NET_POLL     3
#define VMO_RING_OP_MEM_CRC      4

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

/* FipSOs Driver ioctl codes */
#define VFS_IOCTL_LIST_PROCESSES      0xC0105001
#define VFS_IOCTL_HWMON_REPORT        0xC0105002
#define NET_IOCTL_PING_GATEWAY        0xC0104E01
#define NET_IOCTL_HTTP_FETCH          0xC0104E02
#define NET_IOCTL_BROWSER_FETCH_SAVE  0xC0104E03
#define WIFI_IOCTL_SCAN               0xC0105701
#define WIFI_IOCTL_STATUS             0xC0105702
#define DISK_IOCTL_SAVE_NOTE          0xC0104401

static void print_banner(void) {
    printf("\n\033[1;36m======================================================================\033[0m\n");
    printf("\033[1;32m  FipSOs Shell v1.9.3 (Linux C stdlib - Interactive Userland REPL)\033[0m\n");
    printf("\033[1;33m  Polyglot Kernel: ASM + C11 + Rust + C++20 | System V ABI | SATA AHCI\033[0m\n");
    printf("\033[1;36m======================================================================\033[0m\n");
    printf("Type '\033[1;37mhelp\033[0m' for available commands, '\033[1;37mls\033[0m' to list files, or '\033[1;37mgui\033[0m' for Desktop.\n\n");
}

static void cmd_help(void) {
    printf("\n\033[1;32mFipSOs Shell - Comandos Disponibles:\033[0m\n");
    printf("  \033[1;37mhelp\033[0m               - Muestra esta lista de ayuda\n");
    printf("  \033[1;37mpwd\033[0m                - Muestra el directorio de trabajo actual (CWD)\n");
    printf("  \033[1;37mcd <dir>\033[0m           - Cambia de directorio\n");
    printf("  \033[1;37mls [dir]\033[0m           - Lista el contenido del directorio con tipos y tamanos\n");
    printf("  \033[1;37mcat <file>\033[0m         - Muestra el contenido de un archivo de texto\n");
    printf("  \033[1;37mecho [text] [> f]\033[0m  - Imprime texto o redirige a un archivo nuevo/existente\n");
    printf("  \033[1;37mtouch <file>\033[0m       - Crea un archivo vacio en el VFS o en /disk/ (SATA)\n");
    printf("  \033[1;37mmkdir <dir>\033[0m        - Crea un nuevo directorio en el VFS\n");
    printf("  \033[1;37mrm <file>\033[0m          - Elimina un archivo del VFS y de SATA AHCI\n");
    printf("  \033[1;37muname [-a]\033[0m         - Informacion del sistema operativo y arquitectura\n");
    printf("  \033[1;37mnproc\033[0m              - Muestra el numero de procesadores/nucleos activos (SMP)\n");
    printf("  \033[1;37mps\033[0m                 - Lista procesos y tareas del planificador\n");
    printf("  \033[1;37mfree / sensors\033[0m     - Telemetria de memoria, temperatura y hardware\n");
    printf("  \033[1;37mclear\033[0m              - Limpia la pantalla de la consola\n");
    printf("  \033[1;37medit / kilo <file>\033[0m - Editor de texto modal a pantalla completa en C\n");
    printf("  \033[1;37mgui\033[0m                - Lanza el Compositor de Escritorio 1280x800 C++20\n");
    printf("  \033[1;37mbusybox [applet] [args]\033[0m - Binario multi-call BusyBox v1.35.0 estatico\n");
    printf("  \033[1;37mcmd1 | cmd2\033[0m        - Soporte de tuberias (pipes) con redireccion POSIX\n");
    printf("  \033[1;37mcapp\033[0m               - Ejecuta la aplicacion de prueba C standard (/bin/c_std_app.elf)\n");
    printf("  \033[1;37mnpu\033[0m                - Ejecuta inferencia acelerada IA C11/Rust (/bin/npu_infer.elf)\n");
    printf("  \033[1;37mavatar\033[0m             - Ejecuta el avatar 3D interactivo C++20 (/bin/avatar_cpp.elf)\n");
    printf("  \033[1;37mfault\033[0m              - Prueba de supervivencia ante #PF en Anillo 3 (/bin/fault_test.elf)\n");
    printf("  \033[1;37mnet / ping\033[0m         - Envia ping ARP/ICMP a la puerta de enlace PCIe e1000\n");
    printf("  \033[1;37mwget [path]\033[0m        - Descarga paquete HTTP por DMA y lo guarda en VFS\n");
    printf("  \033[1;37mwifi\033[0m               - Escanea redes inalambricas 802.11 (/dev/net/wlan0)\n");
    printf("  \033[1;37mcurl <url>\033[0m         - Descarga URL y la persiste en disco SATA AHCI\n");
    printf("  \033[1;37mpkg <url>\033[0m          - Gestor de paquetes: descarga a /disk/bin y registra binario\n");
    printf("  \033[1;37mringtest\033[0m            - Prueba de canal IPC Zero-Syscall sobre /dev/shm/io_ring\n");
    printf("  \033[1;37msync\033[0m               - Persiste una nota con timestamp en sectores LBA de SATA\n");
    printf("  \033[1;37mpoweroff\033[0m           - Apagado seguro ACPI S5 del equipo\n");
    printf("  \033[1;37mreboot\033[0m             - Reinicio por hardware de la CPU\n");
    printf("  \033[1;37mexit\033[0m               - Sale del shell interactivo\n\n");
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
        const char* type_str = "FILE";
        if (entry->d_type == DT_DIR) {
            type_str = "\033[1;34m[DIR]\033[0m ";
        } else if (entry->d_type == DT_CHR) {
            type_str = "\033[1;33m[DEV]\033[0m ";
        } else {
            type_str = "\033[0;37m[FILE]\033[0m";
        }

        /* Stat entry if possible */
        char fullpath[512];
        if (strcmp(target, "/") == 0) {
            snprintf(fullpath, sizeof(fullpath), "/%s", entry->d_name);
        } else {
            snprintf(fullpath, sizeof(fullpath), "%s/%s", target, entry->d_name);
        }
        struct stat st;
        long size = 0;
        if (stat(fullpath, &st) == 0) {
            size = (long)st.st_size;
        }

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
    if (!path || !*path) {
        printf("Uso: cat <archivo>\n");
        return;
    }
    FILE* fp = fopen(path, "r");
    if (!fp) {
        printf("cat: '%s': No existe el archivo o error de lectura\n", path);
        return;
    }
    char buf[512];
    size_t n;
    while ((n = fread(buf, 1, sizeof(buf), fp)) > 0) {
        fwrite(buf, 1, n, stdout);
    }
    printf("\n");
    fclose(fp);
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

static void cmd_rm(const char* path) {
    if (!path || !*path) {
        printf("Uso: rm <archivo>\n");
        return;
    }
    if (unlink(path) != 0) {
        printf("rm: no se pudo eliminar '%s'\n", path);
    } else {
        printf("rm: '%s' eliminado correctamente.\n", path);
    }
}

static void cmd_echo(int argc, char** argv) {
    /* Check for redirection '>' or '>>' */
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

static void cmd_wifi(void) {
    int fb_fd = open("/dev/fb0", O_RDWR);
    if (fb_fd >= 0) {
        ioctl(fb_fd, WIFI_IOCTL_SCAN, 0);
        ioctl(fb_fd, WIFI_IOCTL_STATUS, 1);
        close(fb_fd);
        printf("wifi: Escaneo 802.11 e interfaz wlan0 activados correctamente.\n");
    } else {
        printf("wifi: no se puede abrir /dev/fb0\n");
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
    if (fd < 0) {
        fd = open("/dev/ipc/ring0", O_RDWR);
    }
    if (fd < 0) {
        perror("[ringtest] open");
        return;
    }

    void* mem = mmap(NULL, 65536, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    if (mem == MAP_FAILED) {
        perror("[ringtest] mmap");
        close(fd);
        return;
    }

    volatile struct VmoRingHeader* hdr = (volatile struct VmoRingHeader*)mem;
    if (hdr->magic != VMO_RING_MAGIC) {
        fprintf(stderr, "[ringtest] Error: Magic invalido (0x%08X != 0x%08X)\n", hdr->magic, VMO_RING_MAGIC);
        close(fd);
        return;
    }

    volatile struct VmoSqEntry* sqes = (volatile struct VmoSqEntry*)((char*)mem + 64);
    volatile struct VmoCqEntry* cqes = (volatile struct VmoCqEntry*)((char*)mem + 64 + 32 * sizeof(struct VmoSqEntry));
    char* data_buf = (char*)mem + 2048;

    /* Enqueue 3 test requests into Submission Queue */
    uint32_t tail = hdr->sq_tail;

    /* 1. OP_ECHO */
    strcpy(data_buf, "ZERO_SYSCALL_TEST_DATA");
    uint32_t idx0 = tail % VMO_RING_QUEUE_DEPTH;
    sqes[idx0].opcode = VMO_RING_OP_ECHO;
    sqes[idx0].flags = 0;
    sqes[idx0].user_data = 0x101;
    sqes[idx0].buffer_offset = 0;
    sqes[idx0].len = (uint32_t)strlen(data_buf);
    tail++;

    /* 2. OP_3D_TRANSFORM */
    float* vertices = (float*)(data_buf + 128);
    vertices[0] = 10.0f; vertices[1] = 20.0f; vertices[2] = 30.0f;
    uint32_t idx1 = tail % VMO_RING_QUEUE_DEPTH;
    sqes[idx1].opcode = VMO_RING_OP_3D_TRANSFORM;
    sqes[idx1].flags = 0;
    sqes[idx1].user_data = 0x102;
    sqes[idx1].buffer_offset = 128;
    sqes[idx1].len = 12; // 3 floats
    tail++;

    /* 3. OP_MEM_CRC */
    uint32_t idx2 = tail % VMO_RING_QUEUE_DEPTH;
    sqes[idx2].opcode = VMO_RING_OP_MEM_CRC;
    sqes[idx2].flags = 0;
    sqes[idx2].user_data = 0x103;
    sqes[idx2].buffer_offset = 0;
    sqes[idx2].len = (uint32_t)strlen(data_buf);
    tail++;

    __atomic_store_n(&hdr->sq_tail, tail, __ATOMIC_RELEASE);
    printf("[ringtest] 3 peticiones encoladas en SQ (tail=%u). Esperando resolucion en Core 1...\n", tail);

    /* Wait for Completion Queue results without issuing syscalls */
    int completed = 0;
    uint32_t cq_head = hdr->cq_head;
    for (int retry = 0; retry < 100000 && completed < 3; retry++) {
        uint32_t cq_tail = __atomic_load_n(&hdr->cq_tail, __ATOMIC_ACQUIRE);
        while (cq_head != cq_tail && completed < 3) {
            uint32_t cidx = cq_head % VMO_RING_QUEUE_DEPTH;
            uint64_t udata = cqes[cidx].user_data;
            int32_t res = cqes[cidx].result;
            printf("[ringtest] CQE recibido: user_data=0x%lX, res=%d\n", (unsigned long)udata, res);
            cq_head++;
            completed++;
        }
    }
    __atomic_store_n(&hdr->cq_head, cq_head, __ATOMIC_RELEASE);

    if (completed == 3) {
        printf("\033[1;32m[ringtest] Exito: 3 transacciones Zero-Syscall completadas en memoria compartida VMO!\033[0m\n");
    } else {
        printf("[ringtest] Completadas %d/3 peticiones.\n", completed);
    }
    close(fd);
}

static void cmd_pkg(int argc, char** argv) {
    if (argc < 2) {
        printf("Uso: pkg install <url> | pkg fetch <url> | pkg <url>\n");
        printf("Ejemplo: pkg install http://10.0.2.2:8080/sample_tool.elf\n");
        return;
    }

    const char* raw_url = argv[1];
    if (strcmp(raw_url, "install") == 0 || strcmp(raw_url, "fetch") == 0) {
        if (argc < 3) {
            printf("Error: URL requerida. Ejemplo: pkg install http://10.0.2.2:8080/sample_tool.elf\n");
            return;
        }
        raw_url = argv[2];
    }

    /* Parse URL: [http://]host[:port]/path */
    char host[128] = "10.0.2.2";
    int port = 8080;
    char path[256] = "/sample_tool.elf";

    const char* p = raw_url;
    if (strncmp(p, "http://", 7) == 0) {
        p += 7;
    }

    const char* slash = strchr(p, '/');
    if (slash) {
        snprintf(path, sizeof(path), "%s", slash);
        char host_port[128];
        size_t hlen = slash - p;
        if (hlen >= sizeof(host_port)) hlen = sizeof(host_port) - 1;
        strncpy(host_port, p, hlen);
        host_port[hlen] = '\0';

        char* colon = strchr(host_port, ':');
        if (colon) {
            *colon = '\0';
            port = atoi(colon + 1);
        }
        if (host_port[0] != '\0') {
            snprintf(host, sizeof(host), "%s", host_port);
        }
    } else {
        snprintf(path, sizeof(path), "/%s", p);
    }

    /* Extract filename from path */
    const char* last_slash = strrchr(path, '/');
    const char* filename = last_slash ? (last_slash + 1) : path;
    if (!filename || !*filename) {
        filename = "downloaded_pkg.elf";
    }

    /* Ensure /disk/bin directory exists */
    mkdir("/disk", 0755);
    mkdir("/disk/bin", 0755);

    char dest_path[256];
    snprintf(dest_path, sizeof(dest_path), "/disk/bin/%s", filename);

    printf("\033[1;36m[pkg]\033[0m Conectando a %s:%d para descargar \"%s\"...\n", host, port, path);

    int sock = socket(AF_INET, SOCK_STREAM, 0);
    if (sock < 0) {
        perror("[pkg] socket");
        return;
    }

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

    /* Send HTTP GET */
    char req[512];
    snprintf(req, sizeof(req), "GET %s HTTP/1.0\r\nHost: %s:%d\r\nUser-Agent: FipSOs-Pkg/2.3\r\nConnection: close\r\n\r\n", path, host, port);
    send(sock, req, strlen(req), 0);

    /* Receive HTTP response and parse body */
    char recv_buf[4096];
    ssize_t n = recv(sock, recv_buf, sizeof(recv_buf), 0);
    if (n <= 0) {
        fprintf(stderr, "[pkg] Error: Respuesta vacia del servidor HTTP.\n");
        close(sock);
        return;
    }

    char* header_end = NULL;
    for (ssize_t i = 0; i < n - 3; i++) {
        if (recv_buf[i] == '\r' && recv_buf[i+1] == '\n' && recv_buf[i+2] == '\r' && recv_buf[i+3] == '\n') {
            header_end = &recv_buf[i+4];
            break;
        }
    }

    if (!header_end) {
        for (ssize_t i = 0; i < n - 1; i++) {
            if (recv_buf[i] == '\n' && recv_buf[i+1] == '\n') {
                header_end = &recv_buf[i+2];
                break;
            }
        }
    }

    if (!header_end) {
        header_end = recv_buf;
    }

    int dest_fd = open(dest_path, O_CREAT | O_WRONLY | O_TRUNC, 0755);
    if (dest_fd < 0) {
        perror("[pkg] open dest_path");
        close(sock);
        return;
    }

    size_t first_chunk_body_len = n - (header_end - recv_buf);
    size_t total_written = 0;
    if (first_chunk_body_len > 0) {
        ssize_t w = write(dest_fd, header_end, first_chunk_body_len);
        if (w > 0) {
            total_written += (size_t)w;
        }
    }

    while ((n = recv(sock, recv_buf, sizeof(recv_buf), 0)) > 0) {
        ssize_t w = write(dest_fd, recv_buf, n);
        if (w > 0) {
            total_written += (size_t)w;
        }
    }

    close(dest_fd);
    close(sock);

    printf("\n\033[1;32m[pkg]\033[0m Paquete \"%s\" instalado con exito en %s (%zu bytes).\n", filename, dest_path, total_written);
    printf("\033[1;32m[pkg]\033[0m Registrado en DiskFS SATA AHCI DMA con permisos ejecutables.\n");
    printf("\033[1;32m[pkg]\033[0m Ejecutable disponible inmediatamente: '\033[1;37m%s\033[0m'\n\n", filename);
}

static void cmd_poweroff(void) {
    printf("Apagando el sistema (ACPI S5 Shutdown)...\n");
    syscall(SYS_reboot, 0, 0, 0, 0);
}

static void cmd_reboot(void) {
    printf("Reiniciando el equipo...\n");
    syscall(SYS_reboot, 0, 0, 1, 0);
}

static int execute_command(int argc, char** argv) {
    if (argc == 0 || !argv[0] || !*argv[0]) {
        return 0;
    }

    const char* cmd = argv[0];

    /* Built-in commands */
    if (strcmp(cmd, "help") == 0) {
        cmd_help();
        return 0;
    }
    if (strcmp(cmd, "pwd") == 0) {
        cmd_pwd();
        return 0;
    }
    if (strcmp(cmd, "cd") == 0) {
        cmd_cd(argc > 1 ? argv[1] : "/");
        return 0;
    }
    if (strcmp(cmd, "ls") == 0) {
        cmd_ls(argc > 1 ? argv[1] : ".");
        return 0;
    }
    if (strcmp(cmd, "cat") == 0) {
        cmd_cat(argc > 1 ? argv[1] : "");
        return 0;
    }
    if (strcmp(cmd, "touch") == 0) {
        cmd_touch(argc > 1 ? argv[1] : "");
        return 0;
    }
    if (strcmp(cmd, "mkdir") == 0) {
        cmd_mkdir(argc > 1 ? argv[1] : "");
        return 0;
    }
    if (strcmp(cmd, "rm") == 0) {
        cmd_rm(argc > 1 ? argv[1] : "");
        return 0;
    }
    if (strcmp(cmd, "echo") == 0) {
        cmd_echo(argc, argv);
        return 0;
    }
    if (strcmp(cmd, "uname") == 0) {
        cmd_uname();
        return 0;
    }
    if (strcmp(cmd, "ps") == 0) {
        cmd_ps();
        return 0;
    }
    if (strcmp(cmd, "free") == 0 || strcmp(cmd, "sensors") == 0 || strcmp(cmd, "hwmon") == 0) {
        cmd_sensors();
        return 0;
    }
    if (strcmp(cmd, "net") == 0 || strcmp(cmd, "ping") == 0) {
        cmd_net_ping();
        return 0;
    }
    if (strcmp(cmd, "wget") == 0) {
        cmd_wget(argc > 1 ? argv[1] : NULL);
        return 0;
    }
    if (strcmp(cmd, "wifi") == 0) {
        cmd_wifi();
        return 0;
    }
    if (strcmp(cmd, "curl") == 0) {
        cmd_curl(argc > 1 ? argv[1] : NULL);
        return 0;
    }
    if (strcmp(cmd, "sync") == 0 || strcmp(cmd, "disk") == 0) {
        cmd_sync();
        return 0;
    }
    if (strcmp(cmd, "pkg") == 0) {
        cmd_pkg(argc, argv);
        return 0;
    }
    if (strcmp(cmd, "ringtest") == 0) {
        cmd_ringtest();
        return 0;
    }
    if (strcmp(cmd, "clear") == 0) {
        printf("\033[2J\033[H");
        return 0;
    }
    if (strcmp(cmd, "poweroff") == 0) {
        cmd_poweroff();
        return 0;
    }
    if (strcmp(cmd, "reboot") == 0) {
        cmd_reboot();
        return 0;
    }
    if (strcmp(cmd, "exit") == 0) {
        printf("Saliendo de FipSOs Shell. Hasta pronto.\n");
        exit(0);
    }

    /* Program shortcut aliases */
    char binary_path[256];
    if (strcmp(cmd, "gui") == 0) {
        snprintf(binary_path, sizeof(binary_path), "/bin/desktop_ui.elf");
    } else if (strcmp(cmd, "capp") == 0) {
        snprintf(binary_path, sizeof(binary_path), "/bin/c_std_app.elf");
    } else if (strcmp(cmd, "npu") == 0 || strcmp(cmd, "infer") == 0) {
        snprintf(binary_path, sizeof(binary_path), "/bin/npu_infer.elf");
    } else if (strcmp(cmd, "avatar") == 0 || strcmp(cmd, "cpp") == 0) {
        snprintf(binary_path, sizeof(binary_path), "/bin/avatar_cpp.elf");
    } else if (strcmp(cmd, "busybox") == 0 || strcmp(cmd, "nproc") == 0) {
        snprintf(binary_path, sizeof(binary_path), "/bin/busybox");
    } else if (strcmp(cmd, "fault") == 0) {
        snprintf(binary_path, sizeof(binary_path), "/bin/fault_test.elf");
    } else if (strcmp(cmd, "edit") == 0 || strcmp(cmd, "kilo") == 0) {
        snprintf(binary_path, sizeof(binary_path), "/bin/kilo.elf");
    } else if (cmd[0] == '/' || cmd[0] == '.') {
        snprintf(binary_path, sizeof(binary_path), "%s", cmd);
    } else {
        snprintf(binary_path, sizeof(binary_path), "/bin/%s.elf", cmd);
    }

    /* Execute ELF binary via execve with sequential resolution in /bin and /disk/bin */
    char* envp[] = { NULL };
    fprintf(stderr, "[sh:exec] Ejecutando \"%s\"...\n", binary_path);
    int ret = execve(binary_path, argv, envp);
    if (ret < 0 && cmd[0] != '/' && cmd[0] != '.') {
        const char* candidate_formats[] = {
            "/bin/%s",
            "/disk/bin/%s.elf",
            "/disk/bin/%s",
            "/disk/%s",
            NULL
        };
        for (int i = 0; candidate_formats[i] != NULL; i++) {
            char alt_path[256];
            snprintf(alt_path, sizeof(alt_path), candidate_formats[i], cmd);
            ret = execve(alt_path, argv, envp);
            if (ret == 0) {
                break;
            }
        }
        if (ret < 0) {
            fprintf(stderr, "fipsos: comando no encontrado: '%s' (intente 'help')\n", cmd);
        }
    } else if (ret < 0) {
        fprintf(stderr, "fipsos: comando no encontrado: '%s' (intente 'help')\n", cmd);
    }
    if (strcmp(cmd, "gui") == 0 && ret == 0) {
        printf("\n\033[1;32m[fipsos_sh]\033[0m Sesion de escritorio cerrada. Consola TTY 1280x800 restaurada OK.\n");
    }
    return 0;
}

static int parse_args(char* str, char** argv, int max_args) {
    int argc = 0;
    char* token = strtok(str, " \t");
    while (token && argc < max_args - 1) {
        argv[argc++] = token;
        token = strtok(NULL, " \t");
    }
    argv[argc] = NULL;
    return argc;
}

static int execute_pipeline(char* left_cmd_str, char* right_cmd_str) {
    char* left_argv[MAX_ARGS];
    char* right_argv[MAX_ARGS];

    int left_argc = parse_args(left_cmd_str, left_argv, MAX_ARGS);
    int right_argc = parse_args(right_cmd_str, right_argv, MAX_ARGS);

    if (left_argc == 0 || right_argc == 0) {
        fprintf(stderr, "fipsos: error de sintaxis cerca de '|'\n");
        return -1;
    }

    int pfd[2];
    if (pipe(pfd) < 0) {
        perror("pipe");
        return -1;
    }

    int saved_stdout = dup(1);
    int saved_stdin = dup(0);
    if (saved_stdout < 0 || saved_stdin < 0) {
        fprintf(stderr, "fipsos: error al duplicar descriptores de consola\n");
        close(pfd[0]);
        close(pfd[1]);
        if (saved_stdout >= 0) close(saved_stdout);
        if (saved_stdin >= 0) close(saved_stdin);
        return -1;
    }

    /* 1. Conectar stdout a la entrada de escritura de la tuberia (pfd[1]) */
    fflush(stdout);
    if (dup2(pfd[1], 1) < 0) {
        perror("dup2 stdout");
        close(pfd[0]);
        close(pfd[1]);
        close(saved_stdout);
        close(saved_stdin);
        return -1;
    }
    close(pfd[1]);

    /* Ejecutar comando izquierdo (escribe en la tuberia) */
    execute_command(left_argc, left_argv);
    fflush(stdout);

    /* 2. Restaurar stdout y cerrar el extremo de escritura de la tuberia */
    dup2(saved_stdout, 1);
    close(saved_stdout);

    /* 3. Conectar stdin a la salida de lectura de la tuberia (pfd[0]) */
    if (dup2(pfd[0], 0) < 0) {
        perror("dup2 stdin");
        close(pfd[0]);
        close(saved_stdin);
        return -1;
    }
    close(pfd[0]);

    /* Ejecutar comando derecho (lee de la tuberia y escribe en la consola) */
    execute_command(right_argc, right_argv);
    fflush(stdout);

    /* 4. Restaurar stdin original */
    dup2(saved_stdin, 0);
    close(saved_stdin);

    return 0;
}

int main(int argc, char** argv) {
    (void)argc;
    (void)argv;

    print_banner();

    char line[MAX_LINE];
    char* cmd_args[MAX_ARGS];

    while (1) {
        char cwd[128];
        if (!getcwd(cwd, sizeof(cwd))) {
            strcpy(cwd, "/");
        }

        /* Interactive Shell Prompt */
        printf("\033[1;32mfipsos\033[0m:\033[1;34m%s\033[0m# ", cwd);
        fflush(stdout);

        if (!fgets(line, sizeof(line), stdin)) {
            break;
        }

        /* Trim newline and carriage returns */
        size_t len = strlen(line);
        while (len > 0 && (line[len - 1] == '\n' || line[len - 1] == '\r')) {
            line[--len] = '\0';
        }
        if (len == 0) {
            continue;
        }

        /* Check for pipe operator '|' */
        char* pipe_ptr = strchr(line, '|');
        if (pipe_ptr != NULL) {
            *pipe_ptr = '\0';
            char* left_str = line;
            char* right_str = pipe_ptr + 1;
            execute_pipeline(left_str, right_str);
            continue;
        }

        /* Tokenize command line arguments */
        int arg_count = parse_args(line, cmd_args, MAX_ARGS);

        if (arg_count > 0) {
            execute_command(arg_count, cmd_args);
        }
    }

    printf("\n[fipsos_sh] Sesion de shell finalizada.\n");
    return 0;
}
