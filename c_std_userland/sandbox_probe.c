#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <unistd.h>

int main(void) {
    int sock = socket(AF_INET, SOCK_STREAM, 0);
    if (sock < 0) {
        printf("[probe] socket: DENIED (ret=%d errno=%d)\n", sock, errno);
    } else {
        printf("[probe] socket: ALLOWED (fd=%d)\n", sock);
        close(sock);
    }

    int fd_disk = open("/disk/sb_denied.txt", O_WRONLY | O_CREAT | O_TRUNC, 0644);
    if (fd_disk < 0) {
        printf("[probe] write /disk: DENIED (ret=%d errno=%d)\n", fd_disk, errno);
    } else {
        ssize_t nw = write(fd_disk, "ok\n", 3);
        (void)nw;
        close(fd_disk);
        printf("[probe] write /disk: ALLOWED\n");
    }

    mkdir("/mnt/fat0/sandbox", 0755);
    int fd_sb = open("/mnt/fat0/sandbox/sb_ok.txt", O_WRONLY | O_CREAT | O_TRUNC, 0644);
    if (fd_sb < 0) {
        printf("[probe] write /mnt/fat0/sandbox: DENIED (ret=%d errno=%d)\n", fd_sb, errno);
    } else {
        const char* msg = "sandbox_ok\n";
        ssize_t nw = write(fd_sb, msg, strlen(msg));
        (void)nw;
        close(fd_sb);
        printf("[probe] write /mnt/fat0/sandbox: ALLOWED\n");
    }
    return 0;
}
