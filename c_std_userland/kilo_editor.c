/*
 * FipSOs - kilo_editor.c
 * Interactive Full-Screen Modal Terminal Text Editor (/bin/kilo.elf / /bin/edit)
 *
 * Implements a lightweight, robust text editor inspired by Kilo:
 * - Pure Standard Linux C11 compiled with gcc -static
 * - Full ANSI VT100 escape sequence rendering (25x80 Framebuffer console)
 * - Navigation via Arrow Keys (Up, Down, Left, Right), Home, End, PageUp, PageDown
 * - Character insertion, newline breaking, backspace / delete
 * - File loading and persistent saving to SATA AHCI disk (/disk/...) via Ctrl-S
 * - Graceful exit via Ctrl-Q
 * - Status bar with filename, line count, cursor position and dirty flag
 *
 * Strictly adheres to ARCHITECTURE.md (0 warnings, pure Standard Linux C11).
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <fcntl.h>
#include <termios.h>
#include <errno.h>
#include <sys/ioctl.h>

#define KILO_VERSION "1.9.2"
#define CTRL_KEY(k) ((k) & 0x1f)

enum EditorKey {
    KEY_BACKSPACE = 127,
    ARROW_LEFT = 1000,
    ARROW_RIGHT,
    ARROW_UP,
    ARROW_DOWN,
    DEL_KEY,
    HOME_KEY,
    END_KEY,
    PAGE_UP,
    PAGE_DOWN
};

typedef struct erow {
    int size;
    char* chars;
} erow;

struct editorConfig {
    int cx, cy;
    int rx;
    int rowoff;
    int coloff;
    int screenrows;
    int screencols;
    int numrows;
    erow* row;
    int dirty;
    char* filename;
    char statusmsg[80];
    struct termios orig_termios;
};

static struct editorConfig E;

static void disableRawMode(void) {
    tcsetattr(STDIN_FILENO, TCSAFLUSH, &E.orig_termios);
    /* Clear screen and reset cursor on exit */
    if (write(STDOUT_FILENO, "\x1b[2J\x1b[H", 7) < 0) {
        /* Ignore error on exit */
    }
}

static void enableRawMode(void) {
    tcgetattr(STDIN_FILENO, &E.orig_termios);
    atexit(disableRawMode);

    struct termios raw = E.orig_termios;
    raw.c_iflag &= ~(BRKINT | ICRNL | INPCK | ISTRIP | IXON);
    raw.c_oflag &= ~(OPOST);
    raw.c_cflag |= (CS8);
    raw.c_lflag &= ~(ECHO | ICANON | IEXTEN | ISIG);
    raw.c_cc[VMIN] = 1;
    raw.c_cc[VTIME] = 0;

    tcsetattr(STDIN_FILENO, TCSAFLUSH, &raw);
}

static int editorReadKey(void) {
    int nread;
    char c;
    while ((nread = read(STDIN_FILENO, &c, 1)) != 1) {
        if (nread == -1 && errno != EAGAIN) {
            return -1;
        }
    }

    if (c == '\x1b') {
        char seq[3];
        if (read(STDIN_FILENO, &seq[0], 1) != 1) return '\x1b';
        if (read(STDIN_FILENO, &seq[1], 1) != 1) return '\x1b';

        if (seq[0] == '[') {
            if (seq[1] >= '0' && seq[1] <= '9') {
                if (read(STDIN_FILENO, &seq[2], 1) != 1) return '\x1b';
                if (seq[2] == '~') {
                    switch (seq[1]) {
                        case '1': return HOME_KEY;
                        case '3': return DEL_KEY;
                        case '4': return END_KEY;
                        case '5': return PAGE_UP;
                        case '6': return PAGE_DOWN;
                        case '7': return HOME_KEY;
                        case '8': return END_KEY;
                    }
                }
            } else {
                switch (seq[1]) {
                    case 'A': return ARROW_UP;
                    case 'B': return ARROW_DOWN;
                    case 'C': return ARROW_RIGHT;
                    case 'D': return ARROW_LEFT;
                    case 'H': return HOME_KEY;
                    case 'F': return END_KEY;
                }
            }
        } else if (seq[0] == 'O') {
            switch (seq[1]) {
                case 'H': return HOME_KEY;
                case 'F': return END_KEY;
            }
        }
        return '\x1b';
    }
    return c;
}

static void editorInsertRow(int at, const char* s, size_t len) {
    if (at < 0 || at > E.numrows) return;
    E.row = realloc(E.row, sizeof(erow) * (E.numrows + 1));
    memmove(&E.row[at + 1], &E.row[at], sizeof(erow) * (E.numrows - at));

    E.row[at].size = (int)len;
    E.row[at].chars = malloc(len + 1);
    memcpy(E.row[at].chars, s, len);
    E.row[at].chars[len] = '\0';
    E.numrows++;
    E.dirty++;
}

static void editorFreeRow(erow* row) {
    free(row->chars);
}

static void editorDelRow(int at) {
    if (at < 0 || at >= E.numrows) return;
    editorFreeRow(&E.row[at]);
    memmove(&E.row[at], &E.row[at + 1], sizeof(erow) * (E.numrows - at - 1));
    E.numrows--;
    E.dirty++;
}

static void editorRowInsertChar(erow* row, int at, int c) {
    if (at < 0 || at > row->size) at = row->size;
    row->chars = realloc(row->chars, row->size + 2);
    memmove(&row->chars[at + 1], &row->chars[at], row->size - at + 1);
    row->size++;
    row->chars[at] = (char)c;
    E.dirty++;
}

static void editorRowAppendString(erow* row, char* s, size_t len) {
    row->chars = realloc(row->chars, row->size + len + 1);
    memcpy(&row->chars[row->size], s, len);
    row->size += (int)len;
    row->chars[row->size] = '\0';
    E.dirty++;
}

static void editorRowDelChar(erow* row, int at) {
    if (at < 0 || at >= row->size) return;
    memmove(&row->chars[at], &row->chars[at + 1], row->size - at);
    row->size--;
    E.dirty++;
}

static void editorInsertChar(int c) {
    if (E.cy == E.numrows) {
        editorInsertRow(E.numrows, "", 0);
    }
    editorRowInsertChar(&E.row[E.cy], E.cx, c);
    E.cx++;
}

static void editorInsertNewline(void) {
    if (E.cx == 0) {
        editorInsertRow(E.cy, "", 0);
    } else {
        erow* row = &E.row[E.cy];
        editorInsertRow(E.cy + 1, &row->chars[E.cx], row->size - E.cx);
        row = &E.row[E.cy];
        row->size = E.cx;
        row->chars[row->size] = '\0';
    }
    E.cy++;
    E.cx = 0;
}

static void editorDelChar(void) {
    if (E.cy == E.numrows) return;
    if (E.cx == 0 && E.cy == 0) return;

    erow* row = &E.row[E.cy];
    if (E.cx > 0) {
        editorRowDelChar(row, E.cx - 1);
        E.cx--;
    } else {
        E.cx = E.row[E.cy - 1].size;
        editorRowAppendString(&E.row[E.cy - 1], row->chars, row->size);
        editorDelRow(E.cy);
        E.cy--;
    }
}

static void editorOpen(const char* filename) {
    free(E.filename);
    E.filename = strdup(filename);

    FILE* fp = fopen(filename, "r");
    if (!fp) {
        snprintf(E.statusmsg, sizeof(E.statusmsg), "[Nuevo Archivo] \"%s\"", filename);
        return;
    }

    char* line = NULL;
    size_t linecap = 0;
    ssize_t linelen;
    while ((linelen = getline(&line, &linecap, fp)) != -1) {
        while (linelen > 0 && (line[linelen - 1] == '\n' || line[linelen - 1] == '\r')) {
            linelen--;
        }
        editorInsertRow(E.numrows, line, linelen);
    }
    free(line);
    fclose(fp);
    E.dirty = 0;
    snprintf(E.statusmsg, sizeof(E.statusmsg), "[Cargado] \"%s\" (%d lineas)", filename, E.numrows);
}

static void editorSave(void) {
    if (E.filename == NULL) {
        snprintf(E.statusmsg, sizeof(E.statusmsg), "Error: No se especifico nombre de archivo");
        return;
    }

    FILE* fp = fopen(E.filename, "w");
    if (!fp) {
        snprintf(E.statusmsg, sizeof(E.statusmsg), "Error al guardar en \"%s\"", E.filename);
        return;
    }

    size_t total_bytes = 0;
    for (int i = 0; i < E.numrows; ++i) {
        if (E.row[i].size > 0) {
            fwrite(E.row[i].chars, 1, E.row[i].size, fp);
            total_bytes += E.row[i].size;
        }
        fputc('\n', fp);
        total_bytes++;
    }
    fclose(fp);
    E.dirty = 0;
    snprintf(E.statusmsg, sizeof(E.statusmsg), "[GUARDADO] %zu bytes escritos en \"%s\"", total_bytes, E.filename);
}

struct abuf {
    char* b;
    int len;
};

#define ABUF_INIT {NULL, 0}

static void abAppend(struct abuf* ab, const char* s, int len) {
    char* new_b = realloc(ab->b, ab->len + len);
    if (!new_b) return;
    memcpy(&new_b[ab->len], s, len);
    ab->b = new_b;
    ab->len += len;
}

static void abFree(struct abuf* ab) {
    free(ab->b);
}

static void editorScroll(void) {
    E.rx = E.cx;
    if (E.cy < E.rowoff) {
        E.rowoff = E.cy;
    }
    if (E.cy >= E.rowoff + E.screenrows) {
        E.rowoff = E.cy - E.screenrows + 1;
    }
    if (E.rx < E.coloff) {
        E.coloff = E.rx;
    }
    if (E.rx >= E.coloff + E.screencols) {
        E.coloff = E.rx - E.screencols + 1;
    }
}

static void editorDrawRows(struct abuf* ab) {
    for (int y = 0; y < E.screenrows; ++y) {
        int filerow = y + E.rowoff;
        if (filerow >= E.numrows) {
            if (E.numrows == 0 && y == E.screenrows / 3) {
                char welcome[80];
                int welcomelen = snprintf(welcome, sizeof(welcome),
                    "FipSOs Kilo Editor -- Version %s", KILO_VERSION);
                if (welcomelen > E.screencols) welcomelen = E.screencols;
                int padding = (E.screencols - welcomelen) / 2;
                if (padding) {
                    abAppend(ab, "~", 1);
                    padding--;
                }
                while (padding--) abAppend(ab, " ", 1);
                abAppend(ab, welcome, welcomelen);
            } else {
                abAppend(ab, "~\x1b[K", 4);
            }
        } else {
            int len = E.row[filerow].size - E.coloff;
            if (len < 0) len = 0;
            if (len > E.screencols) len = E.screencols;
            if (len > 0) {
                abAppend(ab, &E.row[filerow].chars[E.coloff], len);
            }
            abAppend(ab, "\x1b[K", 3);
        }
        abAppend(ab, "\r\n", 2);
    }
}

static void editorDrawStatusBar(struct abuf* ab) {
    abAppend(ab, "\x1b[7m", 4); // Invert colors
    char status[80], rstatus[80];
    int len = snprintf(status, sizeof(status), " %s %s - %d lineas",
        E.filename ? E.filename : "[Sin Nombre]",
        E.dirty ? "(modificado)" : "",
        E.numrows);
    int rlen = snprintf(rstatus, sizeof(rstatus), "Ln %d, Col %d ", E.cy + 1, E.cx + 1);
    if (len > E.screencols) len = E.screencols;
    abAppend(ab, status, len);
    while (len < E.screencols) {
        if (E.screencols - len == rlen) {
            abAppend(ab, rstatus, rlen);
            break;
        } else {
            abAppend(ab, " ", 1);
            len++;
        }
    }
    abAppend(ab, "\x1b[m\r\n", 5);
}

static void editorDrawMessageBar(struct abuf* ab) {
    abAppend(ab, "\x1b[K", 3);
    char help_msg[128];
    snprintf(help_msg, sizeof(help_msg), "Ctrl-S: Guardar en SATA | Ctrl-Q: Salir | %s", E.statusmsg);
    int msglen = (int)strlen(help_msg);
    if (msglen > E.screencols) msglen = E.screencols;
    abAppend(ab, help_msg, msglen);
}

static void editorRefreshScreen(void) {
    editorScroll();
    struct abuf ab = ABUF_INIT;

    abAppend(&ab, "\x1b[?25l", 6); // Hide cursor
    abAppend(&ab, "\x1b[H", 3);    // Go home

    editorDrawRows(&ab);
    editorDrawStatusBar(&ab);
    editorDrawMessageBar(&ab);

    char buf[32];
    snprintf(buf, sizeof(buf), "\x1b[%d;%dH", (E.cy - E.rowoff) + 1, (E.rx - E.coloff) + 1);
    abAppend(&ab, buf, (int)strlen(buf));

    abAppend(&ab, "\x1b[?25h", 6); // Show cursor

    if (write(STDOUT_FILENO, ab.b, ab.len) < 0) {
        /* Redraw error */
    }
    abFree(&ab);
}

static void editorMoveCursor(int key) {
    erow* row = (E.cy >= E.numrows) ? NULL : &E.row[E.cy];
    switch (key) {
        case ARROW_LEFT:
            if (E.cx != 0) {
                E.cx--;
            } else if (E.cy > 0) {
                E.cy--;
                E.cx = E.row[E.cy].size;
            }
            break;
        case ARROW_RIGHT:
            if (row && E.cx < row->size) {
                E.cx++;
            } else if (row && E.cx == row->size) {
                E.cy++;
                E.cx = 0;
            }
            break;
        case ARROW_UP:
            if (E.cy != 0) E.cy--;
            break;
        case ARROW_DOWN:
            if (E.cy < E.numrows) E.cy++;
            break;
    }

    row = (E.cy >= E.numrows) ? NULL : &E.row[E.cy];
    int rowlen = row ? row->size : 0;
    if (E.cx > rowlen) {
        E.cx = rowlen;
    }
}

static void editorProcessKeypress(void) {
    int c = editorReadKey();
    switch (c) {
        case '\r':
            editorInsertNewline();
            break;
        case CTRL_KEY('q'):
            exit(0);
            break;
        case CTRL_KEY('s'):
            editorSave();
            break;
        case HOME_KEY:
            E.cx = 0;
            break;
        case END_KEY:
            if (E.cy < E.numrows) E.cx = E.row[E.cy].size;
            break;
        case KEY_BACKSPACE:
        case 8:
            editorDelChar();
            break;
        case DEL_KEY:
            editorMoveCursor(ARROW_RIGHT);
            editorDelChar();
            break;
        case PAGE_UP:
        case PAGE_DOWN:
            {
                if (c == PAGE_UP) {
                    E.cy = E.rowoff;
                } else if (c == PAGE_DOWN) {
                    E.cy = E.rowoff + E.screenrows - 1;
                    if (E.cy > E.numrows) E.cy = E.numrows;
                }
                int times = E.screenrows;
                while (times--) {
                    editorMoveCursor(c == PAGE_UP ? ARROW_UP : ARROW_DOWN);
                }
            }
            break;
        case ARROW_UP:
        case ARROW_DOWN:
        case ARROW_LEFT:
        case ARROW_RIGHT:
            editorMoveCursor(c);
            break;
        case '\x1b':
            break;
        default:
            editorInsertChar(c);
            break;
    }
}

static void initEditor(void) {
    E.cx = 0;
    E.cy = 0;
    E.rx = 0;
    E.rowoff = 0;
    E.coloff = 0;
    E.numrows = 0;
    E.row = NULL;
    E.dirty = 0;
    E.filename = NULL;
    E.statusmsg[0] = '\0';
    E.screenrows = 23; // 25 total - status bar - message bar
    E.screencols = 80;

    struct winsize ws;
    if (ioctl(STDOUT_FILENO, TIOCGWINSZ, &ws) == 0 && ws.ws_col != 0) {
        E.screenrows = ws.ws_row - 2;
        E.screencols = ws.ws_col;
    }
}

int main(int argc, char** argv) {
    enableRawMode();
    initEditor();

    if (argc >= 2) {
        editorOpen(argv[1]);
    } else {
        editorOpen("/disk/nota.txt");
    }

    while (1) {
        editorRefreshScreen();
        editorProcessKeypress();
    }
    return 0;
}
