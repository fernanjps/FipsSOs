#include <errno.h>
#include <fcntl.h>
#include <stdarg.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <time.h>
#include <unistd.h>

extern long __fips_syscall(long num, long a1, long a2, long a3, long a4, long a5, long a6);

#define AT_FDCWD (-100)
#define SYS_READ 0
#define SYS_WRITE 1
#define SYS_OPEN 2
#define SYS_CLOSE 3
#define SYS_FSTAT 5
#define SYS_LSEEK 8
#define SYS_PIPE 22
#define SYS_DUP 32
#define SYS_DUP2 33
#define SYS_NANOSLEEP 35
#define SYS_GETPID 39
#define SYS_FORK 57
#define SYS_EXECVE 59
#define SYS_EXIT 60
#define SYS_FSYNC 74
#define SYS_TRUNCATE 76
#define SYS_FTRUNCATE 77
#define SYS_GETCWD 79
#define SYS_CHDIR 80
#define SYS_MKDIR 83
#define SYS_RMDIR 84
#define SYS_UNLINK 87
#define SYS_CHMOD 90
#define SYS_GETPPID 110
#define SYS_CLOCK_GETTIME 228
#define SYS_NEWFSTATAT 262

int errno = 0;

static long ret_errno(long r) {
    if (r < 0 && r >= -4095) { errno = (int)(-r); return -1; }
    return r;
}

static FILE _stdin_f = {0, 0, 0, -1};
static FILE _stdout_f = {1, 0, 0, -1};
static FILE _stderr_f = {2, 0, 0, -1};
FILE *stdin = &_stdin_f;
FILE *stdout = &_stdout_f;
FILE *stderr = &_stderr_f;

int open(const char *path, int flags, ...) {
    mode_t mode = 0644;
    if (flags & O_CREAT) {
        va_list ap; va_start(ap, flags); mode = (mode_t)va_arg(ap, int); va_end(ap);
    }
    return (int)ret_errno(__fips_syscall(SYS_OPEN, (long)(uintptr_t)path, flags, (long)mode, 0, 0, 0));
}

ssize_t read(int fd, void *buf, size_t count) {
    return (ssize_t)ret_errno(__fips_syscall(SYS_READ, fd, (long)(uintptr_t)buf, (long)count, 0, 0, 0));
}

ssize_t write(int fd, const void *buf, size_t count) {
    return (ssize_t)ret_errno(__fips_syscall(SYS_WRITE, fd, (long)(uintptr_t)buf, (long)count, 0, 0, 0));
}

int close(int fd) { return (int)ret_errno(__fips_syscall(SYS_CLOSE, fd, 0, 0, 0, 0, 0)); }
off_t lseek(int fd, off_t offset, int whence) {
    return (off_t)ret_errno(__fips_syscall(SYS_LSEEK, fd, offset, whence, 0, 0, 0));
}
int fsync(int fd) { return (int)ret_errno(__fips_syscall(SYS_FSYNC, fd, 0, 0, 0, 0, 0)); }
int ftruncate(int fd, off_t length) { return (int)ret_errno(__fips_syscall(SYS_FTRUNCATE, fd, length, 0, 0, 0, 0)); }
int truncate(const char *path, off_t length) { return (int)ret_errno(__fips_syscall(SYS_TRUNCATE, (long)(uintptr_t)path, length, 0, 0, 0, 0)); }
int unlink(const char *path) { return (int)ret_errno(__fips_syscall(SYS_UNLINK, (long)(uintptr_t)path, 0, 0, 0, 0, 0)); }
int rmdir(const char *path) { return (int)ret_errno(__fips_syscall(SYS_RMDIR, (long)(uintptr_t)path, 0, 0, 0, 0, 0)); }
int mkdir(const char *path, mode_t mode) { return (int)ret_errno(__fips_syscall(SYS_MKDIR, (long)(uintptr_t)path, (long)mode, 0, 0, 0, 0)); }
int chmod(const char *path, mode_t mode) { return (int)ret_errno(__fips_syscall(SYS_CHMOD, (long)(uintptr_t)path, (long)mode, 0, 0, 0, 0)); }
int chdir(const char *path) { return (int)ret_errno(__fips_syscall(SYS_CHDIR, (long)(uintptr_t)path, 0, 0, 0, 0, 0)); }
int stat(const char *path, struct stat *st) { return (int)ret_errno(__fips_syscall(SYS_NEWFSTATAT, AT_FDCWD, (long)(uintptr_t)path, (long)(uintptr_t)st, 0, 0, 0)); }
int fstat(int fd, struct stat *st) { return (int)ret_errno(__fips_syscall(SYS_FSTAT, fd, (long)(uintptr_t)st, 0, 0, 0, 0)); }
int isatty(int fd) { return fd >= 0 && fd <= 2 ? 1 : 0; }
int dup(int oldfd) { return (int)ret_errno(__fips_syscall(SYS_DUP, oldfd, 0, 0, 0, 0, 0)); }
int dup2(int oldfd, int newfd) { return (int)ret_errno(__fips_syscall(SYS_DUP2, oldfd, newfd, 0, 0, 0, 0)); }
int pipe(int pipefd[2]) { return (int)ret_errno(__fips_syscall(SYS_PIPE, (long)(uintptr_t)pipefd, 0, 0, 0, 0, 0)); }
pid_t fork(void) { return (pid_t)ret_errno(__fips_syscall(SYS_FORK, 0, 0, 0, 0, 0, 0)); }
pid_t getpid(void) { return (pid_t)__fips_syscall(SYS_GETPID, 0, 0, 0, 0, 0, 0); }
pid_t getppid(void) { return (pid_t)__fips_syscall(SYS_GETPPID, 0, 0, 0, 0, 0, 0); }
int execve(const char *p, char *const argv[], char *const envp[]) {
    return (int)ret_errno(__fips_syscall(SYS_EXECVE, (long)(uintptr_t)p, (long)(uintptr_t)argv, (long)(uintptr_t)envp, 0, 0, 0));
}
char *getcwd(char *buf, size_t size) {
    long r = __fips_syscall(SYS_GETCWD, (long)(uintptr_t)buf, (long)size, 0, 0, 0, 0);
    return r >= 0 ? buf : NULL;
}
int clock_gettime(int clk_id, struct timespec *tp) { return (int)ret_errno(__fips_syscall(SYS_CLOCK_GETTIME, clk_id, (long)(uintptr_t)tp, 0, 0, 0, 0)); }
int nanosleep(const struct timespec *req, struct timespec *rem) { return (int)ret_errno(__fips_syscall(SYS_NANOSLEEP, (long)(uintptr_t)req, (long)(uintptr_t)rem, 0, 0, 0, 0)); }
time_t time(time_t *tloc) { struct timespec ts; clock_gettime(CLOCK_REALTIME, &ts); if (tloc) *tloc = ts.tv_sec; return ts.tv_sec; }
unsigned int sleep(unsigned int sec) { struct timespec ts = {(time_t)sec, 0}; nanosleep(&ts, NULL); return 0; }
int usleep(unsigned int usec) { struct timespec ts = {(time_t)(usec / 1000000), (long)(usec % 1000000) * 1000L}; return nanosleep(&ts, NULL); }

void _exit(int status) {
    __fips_syscall(SYS_EXIT, status, 0, 0, 0, 0, 0);
    for (;;) {}
}
void exit(int status) { _exit(status); }
void _Exit(int status) { _exit(status); }
void abort(void) { _exit(134); }
char *getenv(const char *name) { (void)name; return NULL; }
int remove(const char *path) { return unlink(path); }

FILE *fdopen(int fd, const char *mode) {
    (void)mode;
    if (fd < 0) return NULL;
    FILE *fp = (FILE *)malloc(sizeof(FILE));
    if (!fp) return NULL;
    fp->fd = fd; fp->eof = 0; fp->err = 0; fp->ungetc_ch = -1;
    return fp;
}

FILE *fopen(const char *path, const char *mode) {
    if (!mode || (mode[0] != 'r' && mode[0] != 'w' && mode[0] != 'a')) {
        errno = EINVAL;
        return NULL;
    }
    int flags = O_RDONLY;
    int plus = strchr(mode, '+') != NULL;
    if (mode[0] == 'r') flags = plus ? O_RDWR : O_RDONLY;
    else if (mode[0] == 'w') flags = (plus ? O_RDWR : O_WRONLY) | O_CREAT | O_TRUNC;
    else flags = (plus ? O_RDWR : O_WRONLY) | O_CREAT | O_APPEND;
    int fd = open(path, flags, 0644);
    if (fd < 0) return NULL;
    return fdopen(fd, mode);
}

int fclose(FILE *fp) {
    if (!fp) return 0;
    int r = close(fp->fd);
    if (fp != stdin && fp != stdout && fp != stderr) free(fp);
    return r;
}

size_t fread(void *ptr, size_t size, size_t nmemb, FILE *fp) {
    if (!fp || size == 0 || nmemb == 0) return 0;
    if (size > SIZE_MAX / nmemb) { fp->err = 1; return 0; }
    size_t want = size * nmemb, done = 0;
    char *dst = (char *)ptr;
    if (fp->ungetc_ch >= 0 && want > 0) {
        dst[done++] = (char)fp->ungetc_ch;
        fp->ungetc_ch = -1;
    }
    while (done < want) {
        ssize_t n = read(fp->fd, dst + done, want - done);
        if (n < 0) { fp->err = 1; break; }
        if (n == 0) { fp->eof = 1; break; }
        done += (size_t)n;
    }
    return done / size;
}

size_t fwrite(const void *ptr, size_t size, size_t nmemb, FILE *fp) {
    if (!fp || size == 0 || nmemb == 0) return 0;
    if (size > SIZE_MAX / nmemb) { fp->err = 1; return 0; }
    size_t want = size * nmemb, done = 0;
    const char *src = (const char *)ptr;
    while (done < want) {
        ssize_t n = write(fp->fd, src + done, want - done);
        if (n <= 0) { fp->err = 1; break; }
        done += (size_t)n;
    }
    return done / size;
}

int fgetc(FILE *fp) { unsigned char c; return fread(&c, 1, 1, fp) == 1 ? (int)c : EOF; }
int getc(FILE *fp) { return fgetc(fp); }
int getchar(void) { return fgetc(stdin); }
int ungetc(int c, FILE *fp) { if (!fp || c == EOF) return EOF; fp->ungetc_ch = (unsigned char)c; fp->eof = 0; return c; }
int fputc(int c, FILE *fp) { unsigned char b = (unsigned char)c; return fwrite(&b, 1, 1, fp) == 1 ? b : EOF; }
int putchar(int c) { return fputc(c, stdout); }
int fputs(const char *s, FILE *fp) { size_t l = strlen(s); return fwrite(s, 1, l, fp) == l ? 0 : EOF; }
int puts(const char *s) { if (fputs(s, stdout) == EOF || fputc('\n', stdout) == EOF) return EOF; return 0; }
int fseek(FILE *fp, long off, int w) { if (!fp) return -1; fp->ungetc_ch = -1; fp->eof = 0; return lseek(fp->fd, off, w) < 0 ? -1 : 0; }
long ftell(FILE *fp) {
    if (!fp) return -1;
    long pos = (long)lseek(fp->fd, 0, SEEK_CUR);
    if (pos > 0 && fp->ungetc_ch >= 0) pos--;
    return pos;
}
void rewind(FILE *fp) { fseek(fp, 0, SEEK_SET); }
int fflush(FILE *fp) { (void)fp; return 0; }
int feof(FILE *fp) { return fp ? fp->eof : 1; }
int ferror(FILE *fp) { return fp ? fp->err : 1; }
void clearerr(FILE *fp) { if (fp) { fp->eof = 0; fp->err = 0; } }

char *fgets(char *s, int size, FILE *fp) {
    if (!s || size <= 0 || !fp) return NULL;
    int i = 0;
    while (i < size - 1) {
        int c = fgetc(fp);
        if (c == EOF) { if (i == 0) return NULL; break; }
        s[i++] = (char)c;
        if (c == '\n') break;
    }
    s[i] = '\0';
    return s;
}

typedef struct { char *buf; size_t cap; size_t len; } FmtOut;
static void fmt_ch(FmtOut *o, char c) { if (o->len + 1 < o->cap) o->buf[o->len] = c; o->len++; }
static void fmt_str(FmtOut *o, const char *s, int width, int prec, int left) {
    int l = 0;
    while (s[l] && (prec < 0 || l < prec)) l++;
    if (!left) for (int i = l; i < width; i++) fmt_ch(o, ' ');
    for (int i = 0; i < l; i++) fmt_ch(o, s[i]);
    if (left) for (int i = l; i < width; i++) fmt_ch(o, ' ');
}
static void fmt_uint(FmtOut *o, unsigned long long v, int base, int upper, char sign, int width, char pad, int left) {
    char tmp[32]; int n = 0;
    const char *dig = upper ? "0123456789ABCDEF" : "0123456789abcdef";
    if (v == 0) tmp[n++] = '0';
    else while (v) { tmp[n++] = dig[v % (unsigned)base]; v /= (unsigned)base; }
    int total = n + (sign ? 1 : 0);
    if (!left && pad == '0' && sign) fmt_ch(o, sign);
    if (!left) for (int i = total; i < width; i++) fmt_ch(o, pad);
    if ((left || pad != '0') && sign) fmt_ch(o, sign);
    while (n--) fmt_ch(o, tmp[n]);
    if (left) for (int i = total; i < width; i++) fmt_ch(o, ' ');
}

int vsnprintf(char *buf, size_t size, const char *fmt, va_list ap) {
    FmtOut o = {buf, size, 0};
    for (; *fmt; fmt++) {
        if (*fmt != '%') { fmt_ch(&o, *fmt); continue; }
        fmt++;
        char pad = ' ', plus_sign = 0; int left = 0;
        for (; *fmt == '-' || *fmt == '+' || *fmt == ' ' || *fmt == '0'; fmt++) {
            if (*fmt == '-') left = 1;
            else if (*fmt == '+') plus_sign = '+';
            else if (*fmt == ' ' && !plus_sign) plus_sign = ' ';
            else if (*fmt == '0') pad = '0';
        }
        if (left) pad = ' ';
        int width = 0;
        if (*fmt == '*') { width = va_arg(ap, int); fmt++; if (width < 0) { left = 1; width = -width; pad = ' '; } }
        else while (*fmt >= '0' && *fmt <= '9') { width = width * 10 + (*fmt - '0'); fmt++; }
        int prec = -1;
        if (*fmt == '.') {
            fmt++; prec = 0;
            if (*fmt == '*') { prec = va_arg(ap, int); fmt++; if (prec < 0) prec = -1; }
            else while (*fmt >= '0' && *fmt <= '9') { prec = prec * 10 + (*fmt - '0'); fmt++; }
        }
        int is_long = 0;
        while (*fmt == 'l' || *fmt == 'z' || *fmt == 'h') { if (*fmt == 'l' || *fmt == 'z') is_long++; fmt++; }
        char c = *fmt ? *fmt : '\0';
        if (!c) break;
        if (c == '%') fmt_ch(&o, '%');
        else if (c == 'c') fmt_ch(&o, (char)va_arg(ap, int));
        else if (c == 's') { const char *s = va_arg(ap, const char *); fmt_str(&o, s ? s : "(null)", width, prec, left); }
        else if (c == 'd' || c == 'i') {
            long long v = is_long >= 2 ? va_arg(ap, long long) : (is_long == 1 ? va_arg(ap, long) : (long long)va_arg(ap, int));
            char sign = v < 0 ? '-' : plus_sign;
            unsigned long long uv = v < 0 ? (0ULL - (unsigned long long)v) : (unsigned long long)v;
            fmt_uint(&o, uv, 10, 0, sign, width, pad, left);
        } else if (c == 'u' || c == 'x' || c == 'X' || c == 'p') {
            int base = (c == 'u') ? 10 : 16;
            unsigned long long uv = (c == 'p' || is_long) ? (is_long >= 2 ? va_arg(ap, unsigned long long) : (unsigned long long)va_arg(ap, unsigned long)) : (unsigned long long)va_arg(ap, unsigned int);
            if (c == 'p') fmt_str(&o, "0x", 0, -1, 0);
            fmt_uint(&o, uv, base, c == 'X', 0, width, pad, left);
        } else { fmt_ch(&o, '%'); fmt_ch(&o, c); }
    }
    if (size > 0) buf[o.len < size ? o.len : size - 1] = '\0';
    return (int)o.len;
}

int snprintf(char *buf, size_t size, const char *fmt, ...) { va_list ap; va_start(ap, fmt); int r = vsnprintf(buf, size, fmt, ap); va_end(ap); return r; }
int sprintf(char *buf, const char *fmt, ...) { va_list ap; va_start(ap, fmt); int r = vsnprintf(buf, (size_t)65536, fmt, ap); va_end(ap); return r; }
int vfprintf(FILE *fp, const char *fmt, va_list ap) {
    char b[1024];
    va_list ap2;
    va_copy(ap2, ap);
    int n = vsnprintf(b, sizeof(b), fmt, ap);
    if (n >= (int)sizeof(b)) {
        char *dyn = (char *)malloc((size_t)n + 1);
        if (dyn) {
            vsnprintf(dyn, (size_t)n + 1, fmt, ap2);
            fwrite(dyn, 1, (size_t)n, fp);
            free(dyn);
            va_end(ap2);
            return n;
        }
    }
    va_end(ap2);
    size_t w = (size_t)(n < (int)sizeof(b) ? n : (int)sizeof(b) - 1);
    fwrite(b, 1, w, fp);
    return n;
}
int fprintf(FILE *fp, const char *fmt, ...) { va_list ap; va_start(ap, fmt); int r = vfprintf(fp, fmt, ap); va_end(ap); return r; }
int vprintf(const char *fmt, va_list ap) { return vfprintf(stdout, fmt, ap); }
int printf(const char *fmt, ...) { va_list ap; va_start(ap, fmt); int r = vfprintf(stdout, fmt, ap); va_end(ap); return r; }
void perror(const char *s) { if (s && *s) fprintf(stderr, "%s: errno=%d\n", s, errno); else fprintf(stderr, "errno=%d\n", errno); }
