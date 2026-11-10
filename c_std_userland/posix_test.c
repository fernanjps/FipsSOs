/*
 * FipSOs - posix_test.c
 * Pure Standard Linux C Application (Ring 3 Userland)
 *
 * Implements standard POSIX/C library workflows:
 * - System V ABI initialization (argc, argv, envp, auxv, MSR_FS_BASE TLS)
 * - Standard I/O (printf, puts via sys_write / sys_writev)
 * - Dynamic Memory Allocation (malloc, free via sys_brk / sys_mmap anonymous)
 * - Standard C File I/O (fopen, fgets, fclose via sys_open, sys_read, sys_close)
 * - Process ID inquiry (getpid, getppid via sys_getpid, sys_getppid)
 * - Process Lifecycle Management: fork() + waitpid() with CR3 PML4 memory isolation
 *
 * STRICT REQUIREMENT: No FipSOs proprietary headers included!
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <sys/wait.h>
#include <signal.h>

static volatile int received_sigusr1 = 0;
static void test_sigusr1_handler(int sig) {
    received_sigusr1 = sig;
}

int main(int argc, char** argv) {
    printf("\n======================================================\n");
    printf("[posix_test.c] Linux Standard C Application on FipSOs v3.1\n");
    printf("======================================================\n");

    /* 1. Verify System V ABI parameters */
    pid_t pid = getpid();
    pid_t ppid = getppid();
    printf("[System V ABI] argc = %d | argv[0] = \"%s\" | PID = %d | PPID = %d\n",
           argc, (argc > 0 && argv[0]) ? argv[0] : "(null)", (int)pid, (int)ppid);

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

    /* 4. Real POSIX Process Lifecycle: fork() + waitpid() + CR3 Memory Isolation */
    printf("\n[POSIX Process Lifecycle] Testing fork() + waitpid() with CR3 memory isolation...\n");
    volatile int isolation_canary = 100;
    pid_t parent_pid = getpid();
    printf("[fork] Parent PID = %d | Canary addr = %p (initial val = %d)\n",
           (int)parent_pid, (void*)&isolation_canary, isolation_canary);

    pid_t child_pid = fork();
    if (child_pid < 0) {
        fprintf(stderr, "[fork] ERROR: fork() failed with return value %d!\n", (int)child_pid);
        return 1;
    }

    if (child_pid == 0) {
        /* Child process execution in cloned PML4 */
        pid_t my_pid = getpid();
        pid_t my_ppid = getppid();
        printf("[Child Process] Running! PID = %d | PPID = %d\n", (int)my_pid, (int)my_ppid);
        printf("[Child Process] Modifying isolation_canary from %d to 999 in child PML4...\n", isolation_canary);
        isolation_canary = 999;
        printf("[Child Process] Canary modified to %d. Exiting with status 42...\n", isolation_canary);
        exit(42);
    }

    /* Parent process execution resumed with child_pid */
    printf("[Parent Process] Fork successful! Spawned child PID = %d. Waiting with waitpid()...\n", (int)child_pid);
    int wstatus = 0;
    pid_t waited_pid = waitpid(child_pid, &wstatus, 0);

    printf("[Parent Process] waitpid() returned PID = %d | raw status = 0x%x\n", (int)waited_pid, wstatus);
    if (waited_pid != child_pid) {
        fprintf(stderr, "[Parent Process] ERROR: waitpid returned %d, expected %d!\n", (int)waited_pid, (int)child_pid);
        return 1;
    }

    int exit_code = WIFEXITED(wstatus) ? WEXITSTATUS(wstatus) : -1;
    printf("[Parent Process] Child exit code = %d (expected 42)\n", exit_code);
    if (exit_code != 42) {
        fprintf(stderr, "[Parent Process] ERROR: Unexpected exit code %d!\n", exit_code);
        return 1;
    }

    printf("[Parent Process] Inspecting isolation_canary = %d (expected 100)...\n", isolation_canary);
    if (isolation_canary != 100) {
        fprintf(stderr, "[Parent Process] ERROR: Memory contamination! Canary is %d, expected 100!\n", isolation_canary);
        return 1;
    }
    printf("[Parent Process] SUCCESS: CR3 PML4 memory isolation verified! Child modification did not touch Parent memory.\n");

    /* 5. Anonymous Pipes, dup2 Redirection and POSIX Signals */
    printf("\n[Anonymous Pipes & IPC] Testing pipe() + fork() child-to-parent IPC...\n");
    int pipefd[2];
    if (pipe(pipefd) < 0) {
        perror("[pipe] pipe() failed");
        return 1;
    }
    printf("[Pipe IPC] Allocated pipe: read_fd=%d, write_fd=%d\n", pipefd[0], pipefd[1]);

    pid_t pipe_child_pid = fork();
    if (pipe_child_pid < 0) {
        perror("[pipe] fork() failed");
        return 1;
    }

    if (pipe_child_pid == 0) {
        /* Child writes message into pipe and exits */
        close(pipefd[0]);
        const char child_msg[] = "FIPSOS_V29_PIPE_IPC_OK";
        ssize_t written = write(pipefd[1], child_msg, strlen(child_msg));
        printf("[Pipe Child] Wrote %zd bytes to pipefd[1]: \"%s\"\n", written, child_msg);
        close(pipefd[1]);
        exit(0);
    }

    /* Parent reads from pipe */
    close(pipefd[1]);
    char pipe_read_buf[64];
    memset(pipe_read_buf, 0, sizeof(pipe_read_buf));
    ssize_t bytes_read = read(pipefd[0], pipe_read_buf, sizeof(pipe_read_buf) - 1);
    printf("[Pipe Parent] Read %zd bytes from pipefd[0]: \"%s\"\n", bytes_read, pipe_read_buf);

    if (strcmp(pipe_read_buf, "FIPSOS_V29_PIPE_IPC_OK") != 0) {
        fprintf(stderr, "[Pipe Parent] ERROR: Message mismatch! Got: \"%s\"\n", pipe_read_buf);
        return 1;
    }

    /* Second read must return 0 (EOF) because writer is closed and buffer is empty */
    ssize_t eof_read = read(pipefd[0], pipe_read_buf, sizeof(pipe_read_buf) - 1);
    printf("[Pipe Parent] Second read returned %zd (expected 0 / EOF)\n", eof_read);
    if (eof_read != 0) {
        fprintf(stderr, "[Pipe Parent] ERROR: Expected EOF (0), got %zd!\n", eof_read);
        return 1;
    }
    close(pipefd[0]);

    int pipe_child_status = 0;
    waitpid(pipe_child_pid, &pipe_child_status, 0);
    printf("[Pipe Parent] SUCCESS: Pipe + fork() IPC verified successfully!\n");

    /* 6. Standard Descriptor Redirection with dup2() */
    printf("\n[dup2 Redirection] Testing stdout redirection to pipe via dup2()...\n");
    int redir_pipe[2];
    if (pipe(redir_pipe) < 0) {
        perror("[dup2] pipe() failed");
        return 1;
    }
    int saved_stdout = dup(1);
    if (saved_stdout < 0) {
        perror("[dup2] dup(1) failed");
        return 1;
    }

    // Redirect stdout to redir_pipe[1]
    if (dup2(redir_pipe[1], 1) < 0) {
        perror("[dup2] dup2() failed");
        return 1;
    }
    printf("STDOUT_REDIRECTED_VIA_DUP2_OK");
    fflush(stdout);

    // Restore stdout
    dup2(saved_stdout, 1);
    close(saved_stdout);
    close(redir_pipe[1]);

    char redir_read_buf[64];
    memset(redir_read_buf, 0, sizeof(redir_read_buf));
    ssize_t r_redir = read(redir_pipe[0], redir_read_buf, sizeof(redir_read_buf) - 1);
    (void)r_redir;
    close(redir_pipe[0]);

    printf("[dup2 Redirection] Captured redirected output: \"%s\"\n", redir_read_buf);
    if (strcmp(redir_read_buf, "STDOUT_REDIRECTED_VIA_DUP2_OK") != 0) {
        fprintf(stderr, "[dup2 Redirection] ERROR: Redirection mismatch! Got: \"%s\"\n", redir_read_buf);
        return 1;
    }
    printf("[dup2 Redirection] SUCCESS: dup2() stdout redirection verified successfully!\n");

    /* 7. POSIX Signals with sigaction() + kill() */
    printf("\n[POSIX Signals] Testing sigaction() and kill() with SIGUSR1...\n");
    struct sigaction sa;
    memset(&sa, 0, sizeof(sa));
    sa.sa_handler = test_sigusr1_handler;
    if (sigaction(SIGUSR1, &sa, NULL) < 0) {
        perror("[signals] sigaction failed");
        return 1;
    }
    printf("[POSIX Signals] Registered test_sigusr1_handler for SIGUSR1 (10). Calling kill()...\n");
    if (kill(getpid(), SIGUSR1) < 0) {
        perror("[signals] kill failed");
        return 1;
    }
    printf("[POSIX Signals] kill(getpid(), SIGUSR1) returned. received_sigusr1 = %d (expected 10)\n", received_sigusr1);
    if (received_sigusr1 != SIGUSR1) {
        fprintf(stderr, "[POSIX Signals] ERROR: Handler was not called! received_sigusr1 = %d\n", received_sigusr1);
        return 1;
    }
    printf("[POSIX Signals] SUCCESS: sigaction() + kill() delivered signal 10 to user handler!\n");

    printf("\n[posix_test.c] SUCCESS: All Linux libc standard routines executed with exit=0!\n");
    printf("======================================================\n\n");
    return 0;
}
