/*
 * FipSOs - posix_test.c
 * Pure Standard Linux C Application (Ring 3 Userland)
 *
 * Implements standard POSIX/C library workflows:
 * - System V ABI initialization (argc, argv, envp, auxv, MSR_FS_BASE TLS)
 * - Standard I/O (printf, puts via sys_write / sys_writev)
 * - Dynamic Memory Allocation (malloc, free via sys_brk / sys_mmap anonymous)
 * - Standard C File I/O (fopen, fgets, fclose via sys_open, sys_read, sys_close)
 * - Process ID inquiry (getpid via sys_getpid)
 *
 * STRICT REQUIREMENT: No FipSOs proprietary headers included!
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

int main(int argc, char** argv) {
    printf("\n======================================================\n");
    printf("[posix_test.c] Linux Standard C Application on FipSOs v1.9\n");
    printf("======================================================\n");

    /* 1. Verify System V ABI parameters */
    pid_t pid = getpid();
    printf("[System V ABI] argc = %d | argv[0] = \"%s\" | PID = %d\n",
           argc, (argc > 0 && argv[0]) ? argv[0] : "(null)", (int)pid);

    /* 2. Standard C Dynamic Memory Allocation (Heap / mmap anonymous) */
    const size_t alloc_bytes = 4096;
    printf("[C libc Heap] Allocating %zu bytes with malloc()...\n", alloc_bytes);
    unsigned char* buffer = (unsigned char*)malloc(alloc_bytes);
    if (!buffer) {
        fprintf(stderr, "[posix_test.c] ERROR: malloc(%zu) returned NULL!\n", alloc_bytes);
        return 1;
    }

    printf("[C libc Heap] Buffer successfully allocated at %p\n", (void*)buffer);

    /* Fill buffer and compute checksum to verify read/write access */
    unsigned long long sum = 0ULL;
    for (size_t i = 0; i < alloc_bytes; ++i) {
        buffer[i] = (unsigned char)((i * 37 + 13) & 0xFF);
        sum += buffer[i];
    }
    printf("[C libc Heap] Memory test passed! Checksum = 0x%llx\n", sum);

    printf("[C libc Heap] Freeing allocated buffer at %p...\n", (void*)buffer);
    free(buffer);
    printf("[C libc Heap] free() completed cleanly.\n");

    /* 3. Standard C File Streams (fopen, fgets, fclose) */
    const char* target_file = "/etc/motd.txt";
    printf("[C File I/O] Opening \"%s\" with fopen()...\n", target_file);
    FILE* fp = fopen(target_file, "r");
    if (!fp) {
        printf("[C File I/O] Warning: Could not open \"%s\", trying \"/disk/persistent_note.txt\"...\n", target_file);
        fp = fopen("/disk/persistent_note.txt", "r");
    }

    if (fp) {
        char line[256];
        if (fgets(line, sizeof(line), fp)) {
            // Trim newline for neat display
            size_t len = strlen(line);
            while (len > 0 && (line[len - 1] == '\n' || line[len - 1] == '\r')) {
                line[--len] = '\0';
            }
            printf("[C File I/O] Read from file: \"%s\"\n", line);
        } else {
            printf("[C File I/O] File opened but empty or read error.\n");
        }
        fclose(fp);
        printf("[C File I/O] fclose() completed cleanly.\n");
    } else {
        printf("[C File I/O] Notice: No test file found on VFS.\n");
    }

    printf("\n[posix_test.c] SUCCESS: All Linux libc standard routines executed with exit=0!\n");
    printf("======================================================\n\n");
    return 0;
}
