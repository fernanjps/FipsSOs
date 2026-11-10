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

static int parse_args(char* str, char** argv, int max_args) {
    int argc = 0;
    char* token = strtok(str, " \t");
    while (token && argc < max_args - 1) {
        argv[argc++] = token;
        token = strtok(NULL, " \t");
    }
    argv[argc] = NULL;
    return argc;
}

static int execute_command(int argc, char** argv) {
    if (argc == 0 || !argv[0] || !*argv[0]) {
        return 0;
    }

    /* 1. Try built-in commands first */
    if (handle_builtin(argc, argv)) {
        return 0;
    }

    const char* cmd = argv[0];

    /* 2. Program shortcut aliases */
    char binary_path[256];
    if (strcmp(cmd, "gui") == 0) {
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
    } else if (cmd[0] == '/' || cmd[0] == '.') {
        snprintf(binary_path, sizeof(binary_path), "%s", cmd);
    } else if (strstr(cmd, ".elf") != NULL) {
        snprintf(binary_path, sizeof(binary_path), "/bin/%s", cmd);
    } else {
        snprintf(binary_path, sizeof(binary_path), "/bin/%s.elf", cmd);
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
        int ret = execve(binary_path, argv, envp);
        if (ret < 0 && cmd[0] != '/' && cmd[0] != '.') {
            const char* candidate_formats[] = {
                "/bin/%s",
                "/disk/bin/%s.elf",
                "/disk/bin/%s",
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
        /* Parent process: wait for child to complete */
        int wstatus = 0;
        waitpid(pid, &wstatus, 0);
        if (strcmp(cmd, "gui") == 0) {
            printf("\n\033[1;32m[fipsos_sh]\033[0m Sesion de escritorio cerrada. Consola TTY 1280x800 restaurada OK.\n");
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

        /* Check for pipe operator '|' */
        char* pipe_ptr = strchr(line, '|');
        if (pipe_ptr != NULL) {
            *pipe_ptr = '\0';
            char* left_str = line;
            char* right_str = pipe_ptr + 1;
            execute_pipeline(left_str, right_str);
            continue;
        }

        /* Check for redirection '>' */
        char* redir_ptr = strchr(line, '>');
        if (redir_ptr != NULL && strncmp(line, "echo ", 5) != 0) {
            *redir_ptr = '\0';
            char* file_part = redir_ptr + 1;
            while (*file_part == ' ' || *file_part == '\t') file_part++;
            if (*file_part) {
                int out_fd = open(file_part, O_WRONLY | O_CREAT | O_TRUNC, 0644);
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
