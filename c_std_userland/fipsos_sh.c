/*
 * FipSOs - fipsos_sh.c
 * Pure Standard Linux C Interactive Userland Shell (/bin/sh /bin/fipsos_sh.elf)
 * Modularized Architecture:
 * - sh/sh_readline.h / sh_readline.c: ANSI terminal input & circular history
 * - sh/sh_builtins.h / sh_builtins.c: Shell built-ins & diagnostic tools
 * - fipsos_sh.c: Main REPL, pipe parsing, redirection & process execution
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <fcntl.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <sys/wait.h>
#include "sh/sh_readline.h"
#include "sh/sh_builtins.h"

#define MAX_LINE 512
#define MAX_ARGS 32
#define SYS_FIPS_SANDBOX 450

extern long syscall(long number, ...);

static int parse_args(char* str, char** argv, int max_args) {
    int argc = 0;
    char* p = str;
    while (*p && argc < max_args - 1) {
        while (*p == ' ' || *p == '\t') p++;
        if (!*p) break;
        char* out = p;
        argv[argc++] = out;
        while (*p && *p != ' ' && *p != '\t') {
            if (*p == '\'' || *p == '"') {
                char q = *p++;
                while (*p && *p != q) *out++ = *p++;
                if (*p == q) p++;
            } else {
                *out++ = *p++;
            }
        }
        if (*p) p++;
        *out = '\0';
    }
    argv[argc] = NULL;
    return argc;
}

static int execute_command(int argc, char** argv) {
    if (argc == 0 || !argv[0] || !*argv[0]) {
        return 0;
    }

    int is_bg = 0;
    if (argc > 1 && strcmp(argv[argc - 1], "&") == 0) {
        is_bg = 1;
        argv[--argc] = NULL;
    } else if (argc > 0) {
        size_t alen = strlen(argv[argc - 1]);
        if (alen > 1 && argv[argc - 1][alen - 1] == '&') {
            argv[argc - 1][alen - 1] = '\0';
            is_bg = 1;
        }
    }
    if (argc == 0 || !argv[0] || !*argv[0]) {
        return 0;
    }

    /* 1. Try built-in commands first */
    if (handle_builtin(argc, argv)) {
        return 0;
    }

    int is_sandbox = 0;
    if (strcmp(argv[0], "sandbox") == 0) {
        if (argc < 2) {
            printf("Uso: sandbox <comando> [args...]\n");
            return 0;
        }
        is_sandbox = 1;
        argv++;
        argc--;
    }

    char* bg_argv[MAX_ARGS + 1];
    if (is_bg && strstr(argv[0], "httpd") != NULL) {
        int k = 0;
        int has_bg = 0;
        while (k < argc && k < MAX_ARGS - 1) {
            bg_argv[k] = argv[k];
            if (strcmp(argv[k], "--bg") == 0) has_bg = 1;
            k++;
        }
        if (!has_bg) bg_argv[k++] = "--bg";
        bg_argv[k] = NULL;
        argv = bg_argv;
    }

    const char* cmd = argv[0];

    /* 2. Program shortcut aliases */
    char binary_path[256];
    if (strcmp(cmd, "gui") == 0 || strcmp(cmd, "desktop") == 0) {
        snprintf(binary_path, sizeof(binary_path), "/bin/desktop_ui.elf");
    } else if (strcmp(cmd, "capp") == 0) {
        snprintf(binary_path, sizeof(binary_path), "/bin/c_std_app.elf");
    } else if (strcmp(cmd, "npu") == 0 || strcmp(cmd, "infer") == 0 || strcmp(cmd, "simd") == 0) {
        snprintf(binary_path, sizeof(binary_path), "/bin/npu_infer.elf");
    } else if (strcmp(cmd, "avatar") == 0 || strcmp(cmd, "cpp") == 0) {
        snprintf(binary_path, sizeof(binary_path), "/bin/avatar_cpp.elf");
    } else if (strcmp(cmd, "busybox") == 0 || strcmp(cmd, "nproc") == 0) {
        snprintf(binary_path, sizeof(binary_path), "/bin/busybox");
    } else if (strcmp(cmd, "fault") == 0) {
        snprintf(binary_path, sizeof(binary_path), "/bin/fault_test.elf");
    } else if (strcmp(cmd, "edit") == 0 || strcmp(cmd, "kilo") == 0) {
        snprintf(binary_path, sizeof(binary_path), "/bin/kilo.elf");
    } else if (strcmp(cmd, "sample") == 0 || strcmp(cmd, "sample_tool") == 0) {
        snprintf(binary_path, sizeof(binary_path), "/bin/sample_tool.elf");
    } else if (strcmp(cmd, "ai") == 0) {
        snprintf(binary_path, sizeof(binary_path), "/bin/fips_ai");
    } else if (strcmp(cmd, "docker") == 0) {
        snprintf(binary_path, sizeof(binary_path), "/bin/docker");
    } else if (cmd[0] == '/' || cmd[0] == '.') {
        snprintf(binary_path, sizeof(binary_path), "%s", cmd);
    } else if (strstr(cmd, ".elf") != NULL) {
        snprintf(binary_path, sizeof(binary_path), "/bin/%s", cmd);
    } else {
        /* Try extensionless exact path first, then .elf fallback in child */
        snprintf(binary_path, sizeof(binary_path), "/bin/%s", cmd);
    }

    /* 3. Execute external binary via fork() + execve() + wait4()/waitpid() */
    char* envp[] = { NULL };
    pid_t pid = fork();
    if (pid < 0) {
        /* Fork failed: try direct execution fallback */
        int ret = execve(binary_path, argv, envp);
        if (ret < 0) {
            fprintf(stderr, "fipsos: comando no encontrado: '%s' (intente 'help')\n", cmd);
        }
        return -1;
    } else if (pid == 0) {
        /* Child process */
        if (is_bg) {
            syscall(SYS_FIPS_SANDBOX, 0x1FUL, 2UL); /* SANDBOX_AUTO_REAP */
        }
        if (is_sandbox) {
            /* CAP_FS_READ(1) | CAP_FS_WRITE(2) | CAP_SPAWN(8) = 0x0B, SANDBOX_RESTRICT_FAT0 = 1 */
            syscall(SYS_FIPS_SANDBOX, 0x0BUL, 1UL);
        }
        int ret = execve(binary_path, argv, envp);
        if (ret < 0 && cmd[0] != '/' && cmd[0] != '.') {
            const char* candidate_formats[] = {
                "/disk/bin/%s",
                "/bin/%s.elf",
                "/disk/bin/%s.elf",
                "/disk/%s",
                NULL
            };
            for (int i = 0; candidate_formats[i] != NULL; i++) {
                char alt_path[256];
                snprintf(alt_path, sizeof(alt_path), candidate_formats[i], cmd);
                ret = execve(alt_path, argv, envp);
                if (ret == 0) break;
            }
        }
        if (ret < 0) {
            fprintf(stderr, "fipsos: comando no encontrado: '%s' (intente 'help')\n", cmd);
            exit(127);
        }
        exit(0);
    } else {
        /* Parent process: wait for child unless launched in background with '&' */
        if (is_bg) {
            printf("[bg] PID=%d\n", (int)pid);
        } else {
            int wstatus = 0;
            waitpid(pid, &wstatus, 0);
            if (strcmp(cmd, "gui") == 0 || strcmp(cmd, "desktop") == 0) {
                printf("\n\033[1;32m[fipsos_sh]\033[0m Sesion de escritorio cerrada. Consola TTY 1280x800 restaurada OK.\n");
            }
        }
    }

    return 0;
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

    /* 1. Conectar stdout a pfd[1] */
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

    /* Ejecutar comando izquierdo */
    execute_command(left_argc, left_argv);
    fflush(stdout);

    /* 2. Restaurar stdout */
    dup2(saved_stdout, 1);
    close(saved_stdout);

    /* 3. Conectar stdin a pfd[0] */
    if (dup2(pfd[0], 0) < 0) {
        perror("dup2 stdin");
        close(pfd[0]);
        close(saved_stdin);
        return -1;
    }
    close(pfd[0]);

    /* Ejecutar comando derecho */
    execute_command(right_argc, right_argv);
    fflush(stdout);

    /* 4. Restaurar stdin original */
    dup2(saved_stdin, 0);
    close(saved_stdin);
    clearerr(stdin);

    return 0;
}

int main(int argc, char** argv) {
    (void)argc;
    (void)argv;

    print_banner();

    char line[MAX_LINE];
    char* cmd_args[MAX_ARGS];

    while (1) {
        while (waitpid(-1, NULL, 1) > 0) {}
        char cwd[128];
        if (!getcwd(cwd, sizeof(cwd))) {
            strcpy(cwd, "/");
        }

        /* Interactive Shell Prompt */
        char prompt[256];
        snprintf(prompt, sizeof(prompt), "\033[1;32mfipsos\033[0m:\033[1;34m%s\033[0m# ", cwd);
        printf("%s", prompt);
        fflush(stdout);

        clearerr(stdin);
        if (read_shell_line(line, sizeof(line), prompt) < 0) {
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

        /* Check for pipe operator '|' (first unquoted) */
        char* pipe_ptr = NULL;
        {
            int p_sq = 0, p_dq = 0;
            for (char* p = line; *p; p++) {
                if (*p == '\'' && !p_dq) {
                    p_sq = !p_sq;
                } else if (*p == '"' && !p_sq) {
                    p_dq = !p_dq;
                } else if (*p == '|' && !p_sq && !p_dq) {
                    pipe_ptr = p;
                    break;
                }
            }
        }
        if (pipe_ptr != NULL) {
            *pipe_ptr = '\0';
            char* left_str = line;
            char* right_str = pipe_ptr + 1;
            execute_pipeline(left_str, right_str);
            continue;
        }

        /* Check for redirection '>' or '>>' (rightmost unquoted) */
        char* redir_ptr = NULL;
        int redir_append = 0;
        int in_sq = 0, in_dq = 0;
        for (char* p = line; *p; p++) {
            if (*p == '\'' && !in_dq) {
                in_sq = !in_sq;
            } else if (*p == '"' && !in_sq) {
                in_dq = !in_dq;
            } else if (*p == '>' && !in_sq && !in_dq) {
                redir_ptr = p;
                if (*(p + 1) == '>') {
                    redir_append = 1;
                    p++;
                } else {
                    redir_append = 0;
                }
            }
        }
        if (redir_ptr != NULL) {
            *redir_ptr = '\0';
            char* file_part = redir_ptr + (redir_append ? 2 : 1);
            while (*file_part == ' ' || *file_part == '\t') file_part++;
            size_t flen = strlen(file_part);
            while (flen > 0 && (file_part[flen - 1] == ' ' || file_part[flen - 1] == '\t')) {
                file_part[--flen] = '\0';
            }
            if (*file_part) {
                int out_flags = O_WRONLY | O_CREAT | (redir_append ? O_APPEND : O_TRUNC);
                int out_fd = open(file_part, out_flags, 0644);
                if (out_fd >= 0) {
                    int saved_out = dup(1);
                    dup2(out_fd, 1);
                    close(out_fd);
                    int arg_count = parse_args(line, cmd_args, MAX_ARGS);
                    if (arg_count > 0) {
                        execute_command(arg_count, cmd_args);
                    }
                    fflush(stdout);
                    dup2(saved_out, 1);
                    close(saved_out);
                    printf("fipsos: salida redirigida a '%s'\n", file_part);
                    continue;
                } else {
                    fprintf(stderr, "fipsos: error al abrir '%s' para redireccion\n", file_part);
                }
            }
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
