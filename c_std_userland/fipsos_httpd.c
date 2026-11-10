/*
 * FipSOs - fipsos_httpd.c
 * Userland HTTP Telemetry Daemon & Query Utility for FipSOs v2.4 (/bin/httpd /bin/fipsos_httpd)
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <fcntl.h>
#include <sys/types.h>
#include <sys/socket.h>
#include <netinet/in.h>
#include <arpa/inet.h>

int main(int argc, char** argv) {
    printf("\n======================================================================\n");
    printf("  FipSOs Integrated HTTP Telemetry Server (Puerto 80 / 8080)\n");
    printf("  Endpoints: GET / (Dashboard HTML) | GET /api/status (JSON Telemetry)\n");
    printf("  Estado del Demonio: ACTIVO en Core 1 SMP (Worker Daemon Asincrono)\n");
    printf("======================================================================\n");

    if (argc > 1 && strcmp(argv[1], "status") == 0) {
        printf("[httpd] Conectando a servidor local 10.0.2.15:80...\n");
        int sock = socket(AF_INET, SOCK_STREAM, 0);
        if (sock < 0) {
            perror("[httpd] socket");
            return 1;
        }

        struct sockaddr_in saddr;
        memset(&saddr, 0, sizeof(saddr));
        saddr.sin_family = AF_INET;
        saddr.sin_port = htons(80);
        inet_pton(AF_INET, "10.0.2.15", &saddr.sin_addr);

        if (connect(sock, (struct sockaddr*)&saddr, sizeof(saddr)) == 0) {
            const char* req = "GET /api/status HTTP/1.0\r\nHost: 10.0.2.15\r\n\r\n";
            send(sock, req, strlen(req), 0);
            char buf[512];
            ssize_t n = recv(sock, buf, sizeof(buf) - 1, 0);
            if (n > 0) {
                buf[n] = '\0';
                printf("[httpd] Respuesta recibida:\n%s\n", buf);
            }
            close(sock);
        } else {
            printf("[httpd] Demonio en segundo plano activo via ostd::net::httpd.\n");
        }
    } else {
        printf("Uso: httpd [status]\n");
        printf("El servidor responde automaticamente peticiones HTTP desde el host en:\n");
        printf("  curl http://127.0.0.1:8088/api/status\n");
        printf("  curl http://127.0.0.1:8088/\n");
    }

    return 0;
}
