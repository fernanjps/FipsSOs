#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

static int send_note(int fd, const char* note, size_t len) {
    if (len == 0) return 0;
    ssize_t n = write(fd, note, len);
    return (n > 0) ? 1 : 0;
}

int main(int argc, char** argv) {
    int fd = open("/dev/audio", O_WRONLY);
    if (fd < 0) {
        printf("[beep] error opening /dev/audio\n");
        return 1;
    }
    if (argc >= 3 && strcmp(argv[1], "--melody") == 0 && argv[2][0]) {
        const char* p = argv[2];
        int notes = 0;
        while (*p != '\0') {
            const char* comma = strchr(p, ',');
            size_t len = comma ? (size_t)(comma - p) : strlen(p);
            notes += send_note(fd, p, len);
            if (!comma) break;
            p = comma + 1;
        }
        close(fd);
        printf("[beep] /dev/audio <- melody (%d notas)\n", notes);
        return (notes > 0) ? 0 : 1;
    }
    const char* msg = (argc > 1 && argv[1] && argv[1][0]) ? argv[1] : "880:45";
    int ok = send_note(fd, msg, strlen(msg));
    close(fd);
    if (!ok) {
        printf("[beep] write failed\n");
        return 1;
    }
    printf("[beep] /dev/audio <- %s\n", msg);
    return 0;
}
