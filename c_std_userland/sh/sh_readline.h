#ifndef FIPSOS_SH_READLINE_H
#define FIPSOS_SH_READLINE_H

#include <stddef.h>

#define CMD_HISTORY_MAX 32
#define CMD_HISTORY_LINE_LEN 256

void add_history(const char* cmd);
int read_shell_line(char* buf, size_t max_len, const char* prompt);

#endif /* FIPSOS_SH_READLINE_H */
