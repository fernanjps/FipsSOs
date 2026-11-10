/*
 * FipSOs - pkg.c
 * Standalone Package Manager Utility for FipSOs (/bin/pkg)
 *
 * Downloads ELF binaries over TCP from HTTP host (e.g. 10.0.2.2:8080)
 * directly into /disk/bin/<name> and registers executable permissions.
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <fcntl.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <sys/socket.h>
#include <netinet/in.h>
#include <arpa/inet.h>

int main(int argc, char** argv) {
    if (argc < 2) {
        printf("Uso: pkg install <url> | pkg fetch <url> | pkg <url>\n");
        printf("Ejemplo: pkg install http://10.0.2.2:8080/sample_tool.elf\n");
        return 1;
    }

    const char* raw_url = argv[1];
    if (strcmp(raw_url, "install") == 0 || strcmp(raw_url, "fetch") == 0) {
        if (argc < 3) {
            printf("Error: URL requerida. Ejemplo: pkg install http://10.0.2.2:8080/sample_tool.elf\n");
            return 1;
        }
        raw_url = argv[2];
    }

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

    const char* last_slash = strrchr(path, '/');
    const char* filename = last_slash ? (last_slash + 1) : path;
    if (!filename || !*filename) {
        filename = "downloaded_pkg.elf";
    }

    mkdir("/disk", 0755);
    mkdir("/disk/bin", 0755);

    char dest_path[256];
    snprintf(dest_path, sizeof(dest_path), "/disk/bin/%s", filename);

    printf("[pkg] Conectando a %s:%d para descargar \"%s\"...\n", host, port, path);

    int sock = socket(AF_INET, SOCK_STREAM, 0);
    if (sock < 0) {
        perror("[pkg] socket");
        return 1;
    }

    struct sockaddr_in saddr;
    memset(&saddr, 0, sizeof(saddr));
    saddr.sin_family = AF_INET;
    saddr.sin_port = htons(port);
    inet_pton(AF_INET, host, &saddr.sin_addr);

    if (connect(sock, (struct sockaddr*)&saddr, sizeof(saddr)) < 0) {
        perror("[pkg] connect");
        close(sock);
        return 1;
    }

    char req[512];
    snprintf(req, sizeof(req), "GET %s HTTP/1.0\r\nHost: %s:%d\r\nUser-Agent: FipSOs-Pkg/2.3\r\nConnection: close\r\n\r\n", path, host, port);
    send(sock, req, strlen(req), 0);

    char recv_buf[4096];
    ssize_t n = recv(sock, recv_buf, sizeof(recv_buf), 0);
    if (n <= 0) {
        fprintf(stderr, "[pkg] Error: Respuesta vacia del servidor HTTP.\n");
        close(sock);
        return 1;
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
        return 1;
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

    printf("\n[pkg] Paquete \"%s\" instalado con exito en %s (%zu bytes).\n", filename, dest_path, total_written);
    printf("[pkg] Registrado en DiskFS SATA AHCI DMA con permisos ejecutables.\n");
    printf("[pkg] Ejecutable disponible inmediatamente: '%s'\n\n", filename);
    return 0;
}
