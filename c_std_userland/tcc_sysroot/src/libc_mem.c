#include <ctype.h>
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

extern long __fips_syscall(long num, long a1, long a2, long a3, long a4, long a5, long a6);

#define SYS_MMAP 9
#define SYS_MUNMAP 11
#define PROT_RW 3
#define MAP_ANON_PRIV 0x22

int isdigit(int c) { return c >= '0' && c <= '9'; }
int isupper(int c) { return c >= 'A' && c <= 'Z'; }
int islower(int c) { return c >= 'a' && c <= 'z'; }
int isalpha(int c) { return isupper(c) || islower(c); }
int isalnum(int c) { return isalpha(c) || isdigit(c); }
int isspace(int c) { return c == ' ' || c == '\t' || c == '\n' || c == '\r' || c == '\v' || c == '\f'; }
int isprint(int c) { return c >= 0x20 && c <= 0x7e; }
int isxdigit(int c) { return isdigit(c) || (c >= 'a' && c <= 'f') || (c >= 'A' && c <= 'F'); }
int tolower(int c) { return isupper(c) ? c + 32 : c; }
int toupper(int c) { return islower(c) ? c - 32 : c; }

size_t strlen(const char *s) {
    const char *p = s;
    while (*p) p++;
    return (size_t)(p - s);
}

int strcmp(const char *a, const char *b) {
    while (*a && (*a == *b)) { a++; b++; }
    return (unsigned char)*a - (unsigned char)*b;
}

int strncmp(const char *a, const char *b, size_t n) {
    for (size_t i = 0; i < n; i++) {
        if (a[i] != b[i] || !a[i]) return (unsigned char)a[i] - (unsigned char)b[i];
    }
    return 0;
}

char *strcpy(char *dst, const char *src) {
    char *r = dst;
    while ((*dst++ = *src++)) {}
    return r;
}

char *strncpy(char *dst, const char *src, size_t n) {
    size_t i = 0;
    for (; i < n && src[i]; i++) dst[i] = src[i];
    for (; i < n; i++) dst[i] = '\0';
    return dst;
}

char *strcat(char *dst, const char *src) {
    strcpy(dst + strlen(dst), src);
    return dst;
}

char *strchr(const char *s, int c) {
    while (*s) { if (*s == (char)c) return (char *)s; s++; }
    return (c == 0) ? (char *)s : NULL;
}

char *strrchr(const char *s, int c) {
    const char *last = NULL;
    for (; *s; s++) if (*s == (char)c) last = s;
    return (c == 0) ? (char *)s : (char *)last;
}

char *strstr(const char *hay, const char *needle) {
    if (!*needle) return (char *)hay;
    size_t nl = strlen(needle);
    for (; *hay; hay++) {
        if (*hay == *needle && strncmp(hay, needle, nl) == 0) return (char *)hay;
    }
    return NULL;
}

char *strdup(const char *s) {
    size_t l = strlen(s) + 1;
    char *p = (char *)malloc(l);
    if (p) memcpy(p, s, l);
    return p;
}

void *memcpy(void *dst, const void *src, size_t n) {
    unsigned char *d = (unsigned char *)dst;
    const unsigned char *s = (const unsigned char *)src;
    for (size_t i = 0; i < n; i++) d[i] = s[i];
    return dst;
}

void *memmove(void *dst, const void *src, size_t n) {
    unsigned char *d = (unsigned char *)dst;
    const unsigned char *s = (const unsigned char *)src;
    if (d < s) {
        for (size_t i = 0; i < n; i++) d[i] = s[i];
    } else if (d > s) {
        for (size_t i = n; i > 0; i--) d[i - 1] = s[i - 1];
    }
    return dst;
}

void *memset(void *dst, int c, size_t n) {
    unsigned char *d = (unsigned char *)dst;
    for (size_t i = 0; i < n; i++) d[i] = (unsigned char)c;
    return dst;
}

int memcmp(const void *a, const void *b, size_t n) {
    const unsigned char *x = (const unsigned char *)a;
    const unsigned char *y = (const unsigned char *)b;
    for (size_t i = 0; i < n; i++) {
        if (x[i] != y[i]) return x[i] - y[i];
    }
    return 0;
}

typedef struct AllocHdr {
    size_t magic;
    size_t map_len;
    size_t user_size;
    struct AllocHdr *next;
} AllocHdr;

#define ALLOC_MAGIC 0x464950534D454D31ULL
#define FREE_CACHE_MAX 64
static AllocHdr *g_free_list = NULL;
static size_t g_free_count = 0;

void *malloc(size_t size) {
    if (size == 0) size = 16;
    if (size > SIZE_MAX - sizeof(AllocHdr) - 4095) return NULL;
    size_t total = (sizeof(AllocHdr) + size + 4095) & ~(size_t)4095;
    for (AllocHdr **pp = &g_free_list; *pp; pp = &(*pp)->next) {
        AllocHdr *cur = *pp;
        if (cur->map_len >= total && cur->map_len <= total * 2 + 16384) {
            *pp = cur->next;
            g_free_count--;
            cur->magic = ALLOC_MAGIC;
            cur->user_size = size;
            cur->next = NULL;
            return (void *)(cur + 1);
        }
    }
    long res = __fips_syscall(SYS_MMAP, 0, (long)total, PROT_RW, MAP_ANON_PRIV, -1, 0);
    if (res < 0 && res >= -4095) return NULL;
    AllocHdr *h = (AllocHdr *)(uintptr_t)res;
    h->magic = ALLOC_MAGIC;
    h->map_len = total;
    h->user_size = size;
    h->next = NULL;
    return (void *)(h + 1);
}

void *calloc(size_t nmemb, size_t size) {
    if (nmemb && size > SIZE_MAX / nmemb) return NULL;
    size_t tot = nmemb * size;
    void *p = malloc(tot);
    if (p) memset(p, 0, tot);
    return p;
}

void free(void *ptr) {
    if (!ptr) return;
    AllocHdr *h = ((AllocHdr *)ptr) - 1;
    if (h->magic == ALLOC_MAGIC) {
        h->magic = 0;
        if (h->map_len <= 1024 * 1024 && g_free_count < FREE_CACHE_MAX) {
            h->next = g_free_list;
            g_free_list = h;
            g_free_count++;
            return;
        }
        __fips_syscall(SYS_MUNMAP, (long)(uintptr_t)h, (long)h->map_len, 0, 0, 0, 0);
    }
}

void *realloc(void *ptr, size_t size) {
    if (!ptr) return malloc(size);
    if (size > SIZE_MAX - sizeof(AllocHdr)) return NULL;
    AllocHdr *h = ((AllocHdr *)ptr) - 1;
    if (h->magic != ALLOC_MAGIC) return NULL;
    if (size + sizeof(AllocHdr) <= h->map_len) {
        h->user_size = size;
        return ptr;
    }
    void *np = malloc(size);
    if (!np) return NULL;
    size_t copy = h->user_size < size ? h->user_size : size;
    memcpy(np, ptr, copy);
    free(ptr);
    return np;
}

int abs(int x) { return x < 0 ? -x : x; }
long labs(long x) { return x < 0 ? -x : x; }

long long strtoll(const char *s, char **endptr, int base) {
    const char *orig = s;
    while (isspace((unsigned char)*s)) s++;
    int neg = 0;
    if (*s == '-') { neg = 1; s++; }
    else if (*s == '+') s++;
    if ((base == 0 || base == 16) && s[0] == '0' && (s[1] == 'x' || s[1] == 'X') && isxdigit((unsigned char)s[2])) {
        base = 16; s += 2;
    } else if (base == 0) {
        base = (*s == '0') ? 8 : 10;
    }
    unsigned long long acc = 0;
    int any = 0;
    for (; *s; s++) {
        int d = -1;
        if (*s >= '0' && *s <= '9') d = *s - '0';
        else if (*s >= 'a' && *s <= 'z') d = *s - 'a' + 10;
        else if (*s >= 'A' && *s <= 'Z') d = *s - 'A' + 10;
        if (d < 0 || d >= base) break;
        acc = acc * (unsigned)base + (unsigned)d;
        any = 1;
    }
    if (endptr) *endptr = (char *)(any ? s : orig);
    return neg ? -(long long)acc : (long long)acc;
}

long strtol(const char *s, char **endptr, int base) { return (long)strtoll(s, endptr, base); }
unsigned long strtoul(const char *s, char **endptr, int base) { return (unsigned long)strtoll(s, endptr, base); }
int atoi(const char *s) { return (int)strtoll(s, NULL, 10); }
long atol(const char *s) { return (long)strtoll(s, NULL, 10); }
