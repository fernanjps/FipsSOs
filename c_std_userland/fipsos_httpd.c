/*
 * FipSOs - fipsos_httpd.c
 * Userland HTTP/1.0 File & Telemetry Server for FipSOs v5.5 (/bin/httpd, /bin/fipsos_httpd)
 */

#include <arpa/inet.h>
#include <fcntl.h>
#include <netinet/in.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <unistd.h>
#include "simd_nn_rt.h"

static int64_t g_http_reqs = 0, g_trav_hits = 0, g_err_hits = 0;
static int g_guard_blocks = 0, g_sandboxed = 0;

static void ai_guard_write_state(const char *verdict) {
    int sfd = open("/mnt/fat0/ai_guard.state", O_WRONLY | O_CREAT | O_TRUNC, 0644);
    if (sfd >= 0) {
        char sbuf[64];
        int slen = snprintf(sbuf, sizeof(sbuf), "%s|%s|%d\n", verdict, fnn_simd_backend(), g_guard_blocks);
        if (slen > 0) { ssize_t r = write(sfd, sbuf, (size_t)slen); (void)r; }
        close(sfd);
    }
}

static int ai_guard_eval(const char *uri, int is_trav, int is_err) {
    g_http_reqs++;
    if (is_trav) g_trav_hits++;
    if (is_err)  g_err_hits++;
    int64_t cls = fnn_guard_infer(g_http_reqs, g_trav_hits, g_err_hits, 2, 0, 20);
    if (cls == 2) {
        g_guard_blocks++;
        int lfd = open("/mnt/fat0/security_ai.log", O_WRONLY | O_CREAT | O_APPEND, 0644);
        if (lfd >= 0) {
            char lbuf[256];
            int llen = snprintf(lbuf, sizeof(lbuf),
                "[AI_GUARD] backend=%s verdict=INTRUSION uri=%s blocks=%d us=%lld\n",
                fnn_simd_backend(), uri, g_guard_blocks, (long long)fnn_last_us());
            if (llen > 0) { ssize_t r = write(lfd, lbuf, (size_t)llen); (void)r; }
            close(lfd);
        }
        ai_guard_write_state("INTRUSION");
        int afd = open("/dev/audio", O_WRONLY);
        if (afd >= 0) { ssize_t r = write(afd, "alert", 5); (void)r; close(afd); }
        if (!g_sandboxed) {
            g_sandboxed = 1;
            long ret;
            __asm__ volatile("syscall" : "=a"(ret) : "a"(450L), "D"(0x05L), "S"(0L) : "rcx", "r11", "memory");
        }
        return 2;
    }
    const char *lbl = (cls == 0) ? "SAFE" : "SUSPICIOUS";
    if (g_http_reqs == 1) ai_guard_write_state(lbl);
    printf("[AI_GUARD] backend=%s verdict=%s uri=%s us=%lld\n",
           fnn_simd_backend(), lbl, uri, (long long)fnn_last_us());
    return (int)cls;
}

static void send_all(int fd, const char *buf, size_t len) {
    size_t sent = 0;
    while (sent < len) {
        ssize_t n = send(fd, buf + sent, len - sent, 0);
        if (n <= 0) {
            break;
        }
        sent += (size_t)n;
    }
}

static void send_http_error(int cli, const char *status, const char *msg) {
    char hdr[256];
    int hlen = snprintf(
        hdr,
        sizeof(hdr),
        "HTTP/1.0 %s\r\nContent-Type: text/plain\r\nContent-Length: %zu\r\nConnection: close\r\n\r\n%s",
        status,
        strlen(msg),
        msg
    );
    if (hlen > 0) {
        send_all(cli, hdr, (size_t)hlen);
    }
}

static int has_dotdot(const char *p) {
    return strstr(p, "..") != NULL;
}

static int parse_content_length(const char *hdr) {
    const char *p = hdr;
    while (*p) {
        if (strncasecmp(p, "Content-Length:", 15) == 0) {
            int v = atoi(p + 15);
            return (v > 0 && v <= 65536) ? v : 0;
        }
        const char *nl = strchr(p, '\n');
        if (!nl) break;
        p = nl + 1;
    }
    return 0;
}

static void handle_upload(int cli, const char *req, ssize_t req_len, const char *uri) {
    const char *q = strchr(uri, '?');
    const char *fname = NULL;
    if (q) {
        const char *fp = strstr(q + 1, "file=");
        if (fp) fname = fp + 5;
    }
    char clean[128];
    if (!fname || !*fname) {
        send_http_error(cli, "400 Bad Request", "Missing file= parameter\n");
        close(cli);
        return;
    }
    size_t flen = strcspn(fname, "& \r\n");
    if (flen == 0 || flen >= sizeof(clean)) {
        send_http_error(cli, "400 Bad Request", "Invalid filename\n");
        close(cli);
        return;
    }
    memcpy(clean, fname, flen);
    clean[flen] = '\0';
    if (has_dotdot(clean) || strchr(clean, '/') != NULL) {
        send_http_error(cli, "403 Forbidden", "Path traversal rejected\n");
        close(cli);
        return;
    }

    const char *hdr_end = strstr(req, "\r\n\r\n");
    if (!hdr_end) {
        send_http_error(cli, "400 Bad Request", "Malformed HTTP headers\n");
        close(cli);
        return;
    }
    hdr_end += 4;
    int clen = parse_content_length(req);
    size_t already = (size_t)(req + req_len - hdr_end);

    char path[192];
    snprintf(path, sizeof(path), "/mnt/fat0/%s", clean);
    int out_fd = open(path, O_WRONLY | O_CREAT | O_TRUNC, 0644);
    if (out_fd < 0) {
        send_http_error(cli, "500 Internal Server Error", "Cannot open target file\n");
        close(cli);
        return;
    }

    size_t written = 0;
    size_t first = (clen > 0 && already > (size_t)clen) ? (size_t)clen : already;
    if (first > 0) {
        ssize_t w = write(out_fd, hdr_end, first);
        if (w > 0) written += (size_t)w;
    }
    char buf[1024];
    while (clen > 0 && written < (size_t)clen) {
        size_t want = (size_t)clen - written;
        if (want > sizeof(buf)) want = sizeof(buf);
        ssize_t r = recv(cli, buf, want, 0);
        if (r <= 0) break;
        ssize_t w = write(out_fd, buf, (size_t)r);
        if (w > 0) written += (size_t)w;
    }
    close(out_fd);

    char body[192];
    int blen = snprintf(body, sizeof(body), "{\"ok\":true,\"file\":\"%s\",\"bytes\":%zu}\n", clean, written);
    char hdr[256];
    int hlen = snprintf(
        hdr, sizeof(hdr),
        "HTTP/1.0 200 OK\r\nContent-Type: application/json\r\nContent-Length: %d\r\nConnection: close\r\n\r\n",
        blen
    );
    if (hlen > 0) send_all(cli, hdr, (size_t)hlen < sizeof(hdr) ? (size_t)hlen : sizeof(hdr) - 1);
    if (blen > 0) send_all(cli, body, (size_t)blen < sizeof(body) ? (size_t)blen : sizeof(body) - 1);
    close(cli);
}

static void handle_client(int cli) {
    char req[2048];
    ssize_t n = recv(cli, req, sizeof(req) - 1, 0);
    if (n <= 0) {
        close(cli);
        return;
    }
    req[n] = '\0';

    int is_post = (strncmp(req, "POST ", 5) == 0);
    int is_get  = (strncmp(req, "GET ", 4) == 0);
    if (!is_get && !is_post) {
        send_http_error(cli, "405 Method Not Allowed", "Only GET/POST supported\n");
        close(cli);
        return;
    }

    const char *uri = req + (is_post ? 5 : 4);
    size_t ulen = strcspn(uri, " \r\n\t");
    char raw_uri[256];
    if (ulen >= sizeof(raw_uri)) ulen = sizeof(raw_uri) - 1;
    memcpy(raw_uri, uri, ulen);
    raw_uri[ulen] = '\0';

    if (is_post) {
        if (strncmp(raw_uri, "/upload", 7) == 0) {
            handle_upload(cli, req, n, raw_uri);
        } else {
            send_http_error(cli, "404 Not Found", "Use POST /upload?file=<name>\n");
            close(cli);
        }
        return;
    }

    size_t clen = strcspn(raw_uri, "?");
    char rel[256];
    if (clen >= sizeof(rel)) clen = sizeof(rel) - 1;
    memcpy(rel, raw_uri, clen);
    rel[clen] = '\0';

    if (has_dotdot(rel)) {
        ai_guard_eval(rel, 1, 1);
        send_http_error(cli, "403 Forbidden", "AI Guard: INTRUSION blocked\n");
        close(cli);
        return;
    }
    if (ai_guard_eval(rel, 0, 0) == 2) {
        send_http_error(cli, "403 Forbidden", "AI Guard: INTRUSION blocked\n");
        close(cli);
        return;
    }

    if (strcmp(rel, "/") == 0 || strcmp(rel, "/index.html") == 0) {
        char hwmon[512];
        memset(hwmon, 0, sizeof(hwmon));
        int hfd = open("/sys/fipsos/hwmon", O_RDONLY);
        if (hfd >= 0) {
            ssize_t hr = read(hfd, hwmon, sizeof(hwmon) - 1);
            if (hr > 0) {
                hwmon[hr] = '\0';
            }
            close(hfd);
        } else {
            snprintf(hwmon, sizeof(hwmon), "hwmon unavailable");
        }
        char body[1024];
        int blen = snprintf(
            body,
            sizeof(body),
            "<html><body><h1>FipSOs v5.5 HTTP File Server</h1>"
            "<p>Root: <code>/mnt/fat0/</code> (e.g. <code>GET /hello.lua</code>)</p>"
            "<pre>%s</pre></body></html>\n",
            hwmon
        );
        char hdr[256];
        int hlen = snprintf(
            hdr,
            sizeof(hdr),
            "HTTP/1.0 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: %d\r\nConnection: close\r\n\r\n",
            blen
        );
        if (hlen > 0) {
            send_all(cli, hdr, (size_t)hlen);
        }
        if (blen > 0) {
            send_all(cli, body, (size_t)blen);
        }
        close(cli);
        return;
    }

    const char *clean = (rel[0] == '/') ? (rel + 1) : rel;
    char full_path[300];
    snprintf(full_path, sizeof(full_path), "/mnt/fat0/%s", clean);

    int ffd = open(full_path, O_RDONLY);
    if (ffd < 0) {
        send_http_error(cli, "404 Not Found", "File not found in /mnt/fat0\n");
        close(cli);
        return;
    }

    struct stat st;
    if (fstat(ffd, &st) < 0 || st.st_size < 0) {
        close(ffd);
        send_http_error(cli, "500 Internal Server Error", "fstat failed\n");
        close(cli);
        return;
    }
    size_t fsize = (size_t)st.st_size;

    char hdr[256];
    int hlen = snprintf(
        hdr,
        sizeof(hdr),
        "HTTP/1.0 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: %zu\r\nConnection: close\r\n\r\n",
        fsize
    );
    if (hlen > 0) {
        size_t send_hlen = ((size_t)hlen < sizeof(hdr)) ? (size_t)hlen : sizeof(hdr) - 1;
        send_all(cli, hdr, send_hlen);
    }

    char chunk[4096];
    size_t sent_bytes = 0;
    while (sent_bytes < fsize) {
        size_t want = fsize - sent_bytes;
        if (want > sizeof(chunk)) {
            want = sizeof(chunk);
        }
        ssize_t r = read(ffd, chunk, want);
        if (r <= 0) {
            break;
        }
        send_all(cli, chunk, (size_t)r);
        sent_bytes += (size_t)r;
    }
    close(ffd);
    close(cli);
}

static int serve_http(int port, int max_req) {
    int srv = socket(AF_INET, SOCK_STREAM, 0);
    if (srv < 0) {
        perror("[httpd] socket");
        return 1;
    }
    struct sockaddr_in addr;
    memset(&addr, 0, sizeof(addr));
    addr.sin_family = AF_INET;
    addr.sin_port = htons((uint16_t)port);
    addr.sin_addr.s_addr = htonl(INADDR_ANY);

    if (bind(srv, (struct sockaddr *)&addr, sizeof(addr)) < 0) {
        perror("[httpd] bind");
        close(srv);
        return 1;
    }
    if (listen(srv, 4) < 0) {
        perror("[httpd] listen");
        close(srv);
        return 1;
    }

    printf("[httpd] Escuchando HTTP/1.0 en 0.0.0.0:%d (/mnt/fat0)...\n", port);
    fflush(stdout);

    int handled = 0;
    for (;;) {
        int cli = accept(srv, NULL, NULL);
        if (cli >= 0) {
            handle_client(cli);
            handled++;
            if (max_req > 0 && handled >= max_req) break;
        }
    }
    close(srv);
    return 0;
}

int main(int argc, char **argv) {
    printf("\n======================================================================\n");
    printf("  FipSOs v5.5 HTTP File & Telemetry Server (/mnt/fat0)\n");
    printf("  Uso: httpd serve [puerto] | httpd status\n");
    printf("======================================================================\n");

    if (argc > 1 && strcmp(argv[1], "serve") == 0) {
        int port = 8080;
        int bg = 0;
        int force_fg = 0;
        int max_req = 0;
        for (int i = 2; i < argc; ++i) {
            if (strcmp(argv[i], "--fg") == 0) {
                force_fg = 1;
            } else if (strcmp(argv[i], "--bg") == 0) {
                bg = 1;
            } else if (strcmp(argv[i], "--max") == 0 && i + 1 < argc) {
                max_req = atoi(argv[++i]);
            } else {
                int p = atoi(argv[i]);
                if (p > 0 && p <= 65535) port = p;
            }
        }
        if (bg && !force_fg) {
            int srv = socket(AF_INET, SOCK_STREAM, 0);
            if (srv < 0) return 1;
            struct sockaddr_in addr;
            memset(&addr, 0, sizeof(addr));
            addr.sin_family = AF_INET;
            addr.sin_port = htons((uint16_t)port);
            addr.sin_addr.s_addr = htonl(INADDR_ANY);
            if (bind(srv, (struct sockaddr *)&addr, sizeof(addr)) < 0 || listen(srv, 4) < 0) {
                close(srv);
                return 1;
            }
            uint16_t p16 = (uint16_t)port;
            setsockopt(srv, SOL_SOCKET, 0x4648, &p16, sizeof(p16));
            return 0;
        }
        return serve_http(port, max_req);
    }

    if (argc > 1 && strcmp(argv[1], "status") == 0) {
        printf("[httpd] Conectando a servidor local 10.0.2.15:80...\n");
        int sock = socket(AF_INET, SOCK_STREAM, 0);
        if (sock < 0) {
            perror("[httpd] socket");
            return 1;
        }
        struct sockaddr_in srv;
        memset(&srv, 0, sizeof(srv));
        srv.sin_family = AF_INET;
        srv.sin_port = htons(80);
        inet_pton(AF_INET, "10.0.2.15", &srv.sin_addr);
        if (connect(sock, (struct sockaddr *)&srv, sizeof(srv)) < 0) {
            perror("[httpd] connect");
            close(sock);
            return 1;
        }
        const char *req = "GET /api/status HTTP/1.1\r\nHost: 10.0.2.15\r\nConnection: close\r\n\r\n";
        send(sock, req, strlen(req), 0);
        char buf[512];
        ssize_t n;
        while ((n = recv(sock, buf, sizeof(buf) - 1, 0)) > 0) {
            buf[n] = '\0';
            printf("%s", buf);
        }
        close(sock);
        printf("\n");
    }
    return 0;
}
