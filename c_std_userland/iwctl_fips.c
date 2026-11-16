/*
 * /bin/iwctl_fips - IEEE 802.11 / WPA2-PSK AES-CCMP & DMA Firmware CLI for FipSOs v5.5
 */
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <sys/ioctl.h>
#include <unistd.h>

#define WIFI_IOCTL_LOAD_FW   0xC0105701UL
#define WIFI_IOCTL_WPA2_TEST 0xC0105702UL

static int cmd_status(void) {
    int fd = open("/sys/class/net/wlan0/operstate", O_RDONLY);
    if (fd < 0) {
        printf("[iwctl_fips] wlan0 operstate: error errno=%d\n", errno);
        return 1;
    }
    char buf[64] = {0};
    ssize_t n = read(fd, buf, sizeof(buf) - 1);
    close(fd);
    if (n <= 0) {
        printf("[iwctl_fips] wlan0 operstate: unreadable\n");
        return 1;
    }
    while (n > 0 && (buf[n - 1] == '\n' || buf[n - 1] == '\r')) {
        buf[--n] = '\0';
    }
    printf("[iwctl_fips] wlan0 operstate=%s\n", buf);
    return 0;
}

static int cmd_load_fw(const char* fw_name) {
    int fd = open("/dev/net/eth0", O_RDONLY);
    if (fd < 0) {
        printf("[iwctl_fips] ERROR: /dev/net/eth0 open failed (errno=%d)\n", errno);
        return 1;
    }
    int rc = ioctl(fd, WIFI_IOCTL_LOAD_FW, fw_name);
    int err = errno;
    close(fd);
    if (rc < 0) {
        const char* tag = (err == ENOENT) ? "-ENOENT" : (err == ENODEV ? "-ENODEV" : "-ERR");
        printf("[iwctl_fips] load-fw '%s' failed: %s (errno=%d)\n", fw_name, tag, err);
        return 1;
    }
    printf("[iwctl_fips] load-fw '%s' OK -> DMA staged\n", fw_name);
    return 0;
}

static int cmd_wpa2_test(void) {
    int fd = open("/dev/net/eth0", O_RDONLY);
    if (fd < 0) {
        printf("[iwctl_fips] ERROR: /dev/net/eth0 open failed (errno=%d)\n", errno);
        return 1;
    }
    char out[128] = {0};
    int rc = ioctl(fd, WIFI_IOCTL_WPA2_TEST, out);
    int err = errno;
    close(fd);
    if (rc < 0) {
        printf("[iwctl_fips] wpa2-test FAILED (errno=%d)\n", err);
        return 1;
    }
    printf("[iwctl_fips] %s\n", out);
    return 0;
}

int main(int argc, char** argv) {
    if (argc < 2 || strcmp(argv[1], "status") == 0) {
        return cmd_status();
    }
    if (strcmp(argv[1], "load-fw") == 0) {
        if (argc < 3) {
            printf("Usage: iwctl_fips load-fw <firmware.ucode>\n");
            return 1;
        }
        return cmd_load_fw(argv[2]);
    }
    if (strcmp(argv[1], "wpa2-test") == 0) {
        return cmd_wpa2_test();
    }
    printf("Usage: iwctl_fips <status|load-fw <file>|wpa2-test>\n");
    return 1;
}
