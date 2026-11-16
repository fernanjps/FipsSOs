/*
 * FipSOs v6.3 - /bin/docker CLI
 * Container runtime frontend: UTS/Mount/PID namespaces & Memory Cgroup limits.
 */
#include <errno.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/wait.h>
#include <unistd.h>

extern long syscall(long number, ...);

#define SYS_IOCTL_NR 16L
#define SYS_CHROOT_NR 161L
#define SYS_SETHOSTNAME_NR 170L
#define FIPSOS_IOCTL_CONTAINER_CTL 0xC0105307UL

enum { CTR_STATUS_CREATED = 0, CTR_STATUS_RUNNING = 1, CTR_STATUS_EXITED = 2 };
enum { CTR_OP_CREATE = 1, CTR_OP_ATTACH_SELF = 2, CTR_OP_FINISH = 3, CTR_OP_LIST = 4, CTR_OP_INSPECT = 5, CTR_OP_REMOVE = 6 };

struct container_ctl_req {
    uint32_t op, id, mem_mb;
    int32_t exit_code;
    uint64_t data_ptr;
    char name[32], image[32], root_path[64], hostname[32];
};

struct container_info {
    uint32_t id, status, pid_count;
    int32_t last_exit_code;
    uint64_t init_pid, mem_limit_pages, used_pages, peak_pages;
    char name[32], image[32], root_path[64], hostname[32];
};

_Static_assert(sizeof(struct container_ctl_req) == 184, "container_ctl_req ABI mismatch");
_Static_assert(sizeof(struct container_info) == 208, "container_info ABI mismatch");

static long ctl_call(struct container_ctl_req *req, uint32_t op, uint32_t id) {
    req->op = op;
    req->id = id;
    return syscall(SYS_IOCTL_NR, 1L, (long)FIPSOS_IOCTL_CONTAINER_CTL, (long)(uintptr_t)req);
}

static void copy_field(char *dst, size_t cap, const char *src) {
    memset(dst, 0, cap);
    if (!src) return;
    size_t n = strlen(src);
    memcpy(dst, src, n >= cap ? cap - 1 : n);
}

static const char *status_label(uint32_t st) {
    return (st == CTR_STATUS_RUNNING) ? "Running" : (st == CTR_STATUS_EXITED ? "Exited" : "Created");
}

static int resolve_image_root(const char *img, char *out, size_t cap) {
    if (!img || !*img) return -1;
    if (!strcmp(img, "fipsos/base:6.3") || !strcmp(img, "fipsos/base") || !strcmp(img, "base")) {
        copy_field(out, cap, "/containers/base-rootfs");
        return 0;
    }
    if (!strcmp(img, "fipsos/ai-runtime:6.3") || !strcmp(img, "fipsos/ai-runtime") || !strcmp(img, "ai-runtime")) {
        copy_field(out, cap, "/containers/ai-rootfs");
        return 0;
    }
    if (img[0] == '/') { copy_field(out, cap, img); return 0; }
    return -1;
}

static void parse_target(struct container_ctl_req *req, const char *arg) {
    char *endp = NULL;
    unsigned long v = strtoul(arg, &endp, 10);
    if (endp && *endp == '\0' && v > 0) req->id = (uint32_t)v;
    else copy_field(req->name, sizeof(req->name), arg);
}

static int cmd_images(void) {
    printf("%-24s %-8s %-14s %-26s %s\n", "REPOSITORY", "TAG", "IMAGE ID", "ROOTFS", "MODE");
    printf("%-24s %-8s %-14s %-26s %s\n", "fipsos/base", "6.3", "sha256:63base0", "/containers/base-rootfs", "ZeroCopy-VMO");
    printf("%-24s %-8s %-14s %-26s %s\n", "fipsos/ai-runtime", "6.3", "sha256:63ai000", "/containers/ai-rootfs", "ZeroCopy-VMO");
    return 0;
}

static int cmd_ps(void) {
    struct container_info list[8];
    struct container_ctl_req req;
    memset(&req, 0, sizeof(req));
    req.data_ptr = (uint64_t)(uintptr_t)list;
    long count = ctl_call(&req, CTR_OP_LIST, 0);
    if (count < 0) {
        fprintf(stderr, "docker ps: error leyendo contenedores (errno=%ld)\n", -count);
        return 1;
    }
    printf("%-4s %-14s %-22s %-12s %-10s %-18s %s\n",
           "ID", "NAME", "IMAGE", "HOSTNAME", "STATUS", "MEM (PEAK/LIM)", "PIDS");
    for (long i = 0; i < count; i++) {
        const struct container_info *c = &list[i];
        printf("%-4u %-14s %-22s %-12s %-8s(%d) %llu KiB / %llu MB   %u\n",
               c->id, c->name, c->image, c->hostname, status_label(c->status), c->last_exit_code,
               (unsigned long long)(c->peak_pages * 4ULL),
               (unsigned long long)(c->mem_limit_pages / 256ULL), c->pid_count);
    }
    return 0;
}

static int cmd_inspect(const char *target) {
    if (!target || !*target) { fprintf(stderr, "Uso: docker inspect <id|nombre>\n"); return 1; }
    struct container_info info;
    struct container_ctl_req req;
    memset(&req, 0, sizeof(req));
    req.data_ptr = (uint64_t)(uintptr_t)&info;
    parse_target(&req, target);
    if (ctl_call(&req, CTR_OP_INSPECT, req.id) < 0) {
        fprintf(stderr, "docker inspect: contenedor '%s' no encontrado\n", target);
        return 1;
    }
    printf("{\n  \"Id\": %u,\n  \"Name\": \"%s\",\n  \"Image\": \"%s\",\n  \"RootPath\": \"%s\",\n"
           "  \"Hostname\": \"%s\",\n  \"Status\": \"%s\",\n  \"InitPID\": %llu,\n  \"Pids\": %u,\n"
           "  \"MemLimitPages\": %llu,\n  \"MemLimitMB\": %llu,\n  \"UsedPages\": %llu,\n"
           "  \"PeakPages\": %llu,\n  \"LastExitCode\": %d\n}\n",
           info.id, info.name, info.image, info.root_path, info.hostname, status_label(info.status),
           (unsigned long long)info.init_pid, info.pid_count, (unsigned long long)info.mem_limit_pages,
           (unsigned long long)(info.mem_limit_pages / 256ULL), (unsigned long long)info.used_pages,
           (unsigned long long)info.peak_pages, info.last_exit_code);
    return 0;
}

static int cmd_rm(const char *target) {
    if (!target || !*target) { fprintf(stderr, "Uso: docker rm <id|nombre>\n"); return 1; }
    struct container_ctl_req req;
    memset(&req, 0, sizeof(req));
    parse_target(&req, target);
    long rc = ctl_call(&req, CTR_OP_REMOVE, req.id);
    if (rc < 0) { fprintf(stderr, "docker rm: no se pudo eliminar '%s' (err=%ld)\n", target, -rc); return 1; }
    printf("Contenedor '%s' eliminado.\n", target);
    return 0;
}

static int cmd_run(int argc, char **argv) {
    const char *name = NULL, *hostname = NULL;
    uint32_t mem_mb = 0;
    int idx = 2;
    while (idx < argc && argv[idx][0] == '-') {
        if (!strcmp(argv[idx], "--name") && idx + 1 < argc) name = argv[++idx];
        else if (!strcmp(argv[idx], "--mem") && idx + 1 < argc) mem_mb = (uint32_t)atoi(argv[++idx]);
        else if (!strcmp(argv[idx], "--hostname") && idx + 1 < argc) hostname = argv[++idx];
        else { fprintf(stderr, "docker run: opcion desconocida '%s'\n", argv[idx]); return 1; }
        idx++;
    }
    if (idx + 1 >= argc) {
        fprintf(stderr, "Uso: docker run [--name N] [--mem MB] [--hostname H] <imagen> <cmd> [args...]\n");
        return 1;
    }
    const char *image = argv[idx++];
    char root_path[64];
    if (resolve_image_root(image, root_path, sizeof(root_path)) != 0) {
        fprintf(stderr, "docker run: imagen desconocida '%s' (use 'docker images')\n", image);
        return 1;
    }

    struct container_ctl_req req;
    memset(&req, 0, sizeof(req));
    req.mem_mb = mem_mb;
    copy_field(req.name, sizeof(req.name), name ? name : "");
    copy_field(req.image, sizeof(req.image), image);
    copy_field(req.root_path, sizeof(req.root_path), root_path);
    copy_field(req.hostname, sizeof(req.hostname), hostname ? hostname : (name ? name : ""));
    long cid = ctl_call(&req, CTR_OP_CREATE, 0);
    if (cid <= 0) { fprintf(stderr, "docker run: fallo al crear contenedor (err=%ld)\n", -cid); return 1; }

    char host_buf[32];
    if (hostname && *hostname) copy_field(host_buf, sizeof(host_buf), hostname);
    else if (name && *name) copy_field(host_buf, sizeof(host_buf), name);
    else snprintf(host_buf, sizeof(host_buf), "ctr-%ld", cid);

    pid_t pid = fork();
    if (pid < 0) {
        req.exit_code = 1;
        ctl_call(&req, CTR_OP_FINISH, (uint32_t)cid);
        perror("docker run fork");
        return 1;
    }
    if (pid == 0) {
        struct container_ctl_req sub;
        memset(&sub, 0, sizeof(sub));
        if (ctl_call(&sub, CTR_OP_ATTACH_SELF, (uint32_t)cid) < 0) _exit(125);
        if (syscall(SYS_SETHOSTNAME_NR, host_buf, strlen(host_buf)) < 0) _exit(125);
        if (syscall(SYS_CHROOT_NR, root_path) < 0) _exit(125);
        char *envp[] = { NULL };
        int eret = execve(argv[idx], &argv[idx], envp);
        if (eret >= 0) _exit(0);
        if (argv[idx][0] != '/') {
            char cand[128];
            snprintf(cand, sizeof(cand), "/bin/%s", argv[idx]);
            if (execve(cand, &argv[idx], envp) >= 0) _exit(0);
            snprintf(cand, sizeof(cand), "/bin/%s.elf", argv[idx]);
            if (execve(cand, &argv[idx], envp) >= 0) _exit(0);
        }
        fprintf(stderr, "docker: comando no ejecutable '%s' (errno=%d)\n", argv[idx], errno);
        _exit(errno == ENOMEM ? 137 : 127);
    }

    int wstatus = 0;
    waitpid(pid, &wstatus, 0);
    int exit_code = WIFEXITED(wstatus) ? WEXITSTATUS(wstatus) : 1;
    req.exit_code = exit_code;
    ctl_call(&req, CTR_OP_FINISH, (uint32_t)cid);
    return exit_code;
}

int main(int argc, char **argv) {
    if (argc < 2) {
        printf("FipSOs v6.3 Container Engine (/bin/docker)\n"
               "Uso: docker <images | run [--name N] [--mem MB] [--hostname H] IMG CMD | ps | inspect ID | rm ID>\n");
        return 1;
    }
    if (!strcmp(argv[1], "images")) return cmd_images();
    if (!strcmp(argv[1], "run")) return cmd_run(argc, argv);
    if (!strcmp(argv[1], "ps")) return cmd_ps();
    if (!strcmp(argv[1], "inspect")) return cmd_inspect(argc > 2 ? argv[2] : "");
    if (!strcmp(argv[1], "rm")) return cmd_rm(argc > 2 ? argv[2] : "");
    fprintf(stderr, "docker: subcomando desconocido '%s'\n", argv[1]);
    return 1;
}
