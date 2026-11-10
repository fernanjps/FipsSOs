/*
 * FipSOs - sh_readline.c
 * ANSI Byte-by-Byte Line Reader & 32-Entry Circular Command History
 */

#include "sh_readline.h"
#include <stdio.h>
#include <string.h>
#include <unistd.h>

static char s_cmd_history[CMD_HISTORY_MAX][CMD_HISTORY_LINE_LEN];
static int s_history_count = 0;

void add_history(const char* cmd) {
    if (!cmd || strlen(cmd) == 0) return;
    if (s_history_count > 0) {
        int last_slot = (s_history_count - 1) % CMD_HISTORY_MAX;
        if (strcmp(s_cmd_history[last_slot], cmd) == 0) {
            return;
        }
    }
    int slot = s_history_count % CMD_HISTORY_MAX;
    size_t clen = strlen(cmd);
    if (clen >= CMD_HISTORY_LINE_LEN) {
        clen = CMD_HISTORY_LINE_LEN - 1;
    }
    memcpy(s_cmd_history[slot], cmd, clen);
    s_cmd_history[slot][clen] = '\0';
    s_history_count++;
}

int read_shell_line(char* buf, size_t max_len, const char* prompt) {
    size_t pos = 0;
    buf[0] = '\0';
    int hist_idx = s_history_count;

    while (1) {
        char c = 0;
        int n = read(0, &c, 1);
        if (n <= 0) {
            if (pos > 0) {
                buf[pos] = '\0';
                return (int)pos;
            }
            return -1;
        }

        if (c == '\x1b') {
            char seq[2] = {0, 0};
            if (read(0, &seq[0], 1) > 0 && seq[0] == '[') {
                if (read(0, &seq[1], 1) > 0) {
                    if (seq[1] == 'A') {
                        /* Arrow Up: navigate backward in history */
                        int min_idx = (s_history_count > CMD_HISTORY_MAX) ? (s_history_count - CMD_HISTORY_MAX) : 0;
                        if (hist_idx > min_idx) {
                            hist_idx--;
                            int slot = hist_idx % CMD_HISTORY_MAX;
                            snprintf(buf, max_len, "%s", s_cmd_history[slot]);
                            pos = strlen(buf);
                            printf("\r\033[K%s%s", prompt, buf);
                            fflush(stdout);
                        }
                    } else if (seq[1] == 'B') {
                        /* Arrow Down: navigate forward in history */
                        if (hist_idx < s_history_count) {
                            hist_idx++;
                            if (hist_idx == s_history_count) {
                                buf[0] = '\0';
                                pos = 0;
                            } else {
                                int slot = hist_idx % CMD_HISTORY_MAX;
                                snprintf(buf, max_len, "%s", s_cmd_history[slot]);
                                pos = strlen(buf);
                            }
                            printf("\r\033[K%s%s", prompt, buf);
                            fflush(stdout);
                        }
                    }
                }
            }
            continue;
        }

        if (c == '\n' || c == '\r') {
            if (write(1, "\n", 1) < 0) {}
            buf[pos] = '\0';
            if (pos > 0) {
                add_history(buf);
            }
            return (int)pos;
        }

        if (c == '\b' || c == 0x7F) {
            if (pos > 0) {
                pos--;
                buf[pos] = '\0';
                if (write(1, "\b \b", 3) < 0) {}
            }
            continue;
        }

        if ((unsigned char)c >= 0x20 && (unsigned char)c <= 0x7E) {
            if (pos < max_len - 1) {
                buf[pos++] = c;
                buf[pos] = '\0';
                if (write(1, &c, 1) < 0) {}
            }
        }
    }
}
