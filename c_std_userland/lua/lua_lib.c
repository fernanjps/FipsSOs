#define _POSIX_C_SOURCE 200809L
#include "lua.h"
#include <ctype.h>
#include <math.h>
#include <stdlib.h>
#include <string.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

static void set_global_cfunc(LuaVM* vm, LTable* t, const char* name, LCFunc fn) {
    LStr* ks = lstr_new(vm, name, strlen(name));
    LVal k; k.type = LVAL_STR; k.is_int = 0; k.u.s = ks;
    LVal v; v.type = LVAL_CFUNC; v.is_int = 0; v.u.cf = fn;
    ltab_set(vm, t, k, v);
}

static LVal mk_str(LuaVM* vm, const char* s, size_t len) {
    LVal v; v.type = LVAL_STR; v.is_int = 0; v.u.s = lstr_new(vm, s, len); return v;
}

static int cf_print(LuaVM* vm, int nargs, LVal* args, LVal* out, int max_out) {
    (void)vm; (void)out; (void)max_out;
    for (int i = 0; i < nargs; i++) {
        if (i > 0) putchar('\t');
        if (args[i].type == LVAL_STR) fwrite(args[i].u.s->data, 1, args[i].u.s->len, stdout);
        else { char b[128]; lua_fmt_val(args[i], b, sizeof(b)); fputs(b, stdout); }
    }
    putchar('\n');
    fflush(stdout);
    return 0;
}

static int cf_type(LuaVM* vm, int nargs, LVal* args, LVal* out, int max_out) {
    (void)max_out;
    const char* tn = "nil";
    if (nargs >= 1) {
        switch (args[0].type) {
        case LVAL_BOOL: tn = "boolean"; break;
        case LVAL_NUM: tn = "number"; break;
        case LVAL_STR: tn = "string"; break;
        case LVAL_TABLE: tn = "table"; break;
        case LVAL_FUNC: case LVAL_CFUNC: tn = "function"; break;
        case LVAL_FILE: tn = "userdata"; break;
        default: tn = "nil"; break;
        }
    }
    out[0] = mk_str(vm, tn, strlen(tn));
    return 1;
}

static int cf_tostring(LuaVM* vm, int nargs, LVal* args, LVal* out, int max_out) {
    (void)max_out;
    if (nargs < 1) { out[0] = mk_str(vm, "nil", 3); return 1; }
    if (args[0].type == LVAL_STR) { out[0] = args[0]; return 1; }
    char b[128]; lua_fmt_val(args[0], b, sizeof(b));
    out[0] = mk_str(vm, b, strlen(b));
    return 1;
}

static int cf_tonumber(LuaVM* vm, int nargs, LVal* args, LVal* out, int max_out) {
    (void)vm; (void)max_out;
    if (nargs < 1) { out[0] = lval_nil(); return 1; }
    if (args[0].type == LVAL_NUM) { out[0] = args[0]; return 1; }
    if (args[0].type == LVAL_STR) {
        int base = (nargs >= 2 && args[1].type == LVAL_NUM) ? (int)(args[1].is_int ? args[1].u.i : args[1].u.n) : 10;
        char* end = NULL;
        if (base != 10) {
            long long iv = strtoll(args[0].u.s->data, &end, base);
            if (end != args[0].u.s->data && *end == '\0') { out[0] = lval_int(iv); return 1; }
        } else {
            double d = strtod(args[0].u.s->data, &end);
            if (end != args[0].u.s->data && *end == '\0') {
                if (!strchr(args[0].u.s->data, '.') && !strchr(args[0].u.s->data, 'e') && !strchr(args[0].u.s->data, 'E')) {
                    out[0] = lval_int(strtoll(args[0].u.s->data, NULL, 10));
                } else {
                    out[0] = lval_num(d);
                }
                return 1;
            }
        }
    }
    out[0] = lval_nil();
    return 1;
}

static int cf_assert(LuaVM* vm, int nargs, LVal* args, LVal* out, int max_out) {
    if (nargs < 1 || !lval_truthy(args[0])) {
        const char* msg = (nargs >= 2 && args[1].type == LVAL_STR) ? args[1].u.s->data : "assertion failed!";
        lua_set_error(vm, msg);
        return 0;
    }
    int c = nargs < max_out ? nargs : max_out;
    for (int i = 0; i < c; i++) out[i] = args[i];
    return c;
}

static int cf_error(LuaVM* vm, int nargs, LVal* args, LVal* out, int max_out) {
    (void)out; (void)max_out;
    char msg[128] = "lua error";
    if (nargs >= 1) lua_fmt_val(args[0], msg, sizeof(msg));
    lua_set_error(vm, msg);
    return 0;
}

static int cf_pcall(LuaVM* vm, int nargs, LVal* args, LVal* out, int max_out) {
    if (max_out < 1) return 0;
    if (nargs < 1) {
        out[0] = lval_bool(0);
        if (max_out >= 2) { out[1] = mk_str(vm, "bad argument #1", 15); return 2; }
        return 1;
    }
    LVal res[15];
    int sub_max = max_out > 1 ? (max_out - 1 < 15 ? max_out - 1 : 15) : 0;
    int got = lua_call_val(vm, args[0], nargs - 1, args + 1, res, sub_max);
    if (vm->err_flag) {
        vm->err_flag = 0;
        out[0] = lval_bool(0);
        if (max_out >= 2) { out[1] = mk_str(vm, vm->err_msg, strlen(vm->err_msg)); return 2; }
        return 1;
    }
    out[0] = lval_bool(1);
    int n = 1;
    for (int i = 0; i < got && n < max_out; i++) out[n++] = res[i];
    return n;
}

static int cf_next(LuaVM* vm, int nargs, LVal* args, LVal* out, int max_out) {
    (void)vm;
    if (max_out < 1 || nargs < 1 || args[0].type != LVAL_TABLE) return 0;
    LVal k = nargs >= 2 ? args[1] : lval_nil();
    LVal rk = lval_nil(), rv = lval_nil();
    if (ltab_next(args[0].u.t, k, &rk, &rv)) {
        out[0] = rk;
        if (max_out >= 2) { out[1] = rv; return 2; }
        return 1;
    }
    out[0] = lval_nil(); return 1;
}

static int cf_ipairs_iter(LuaVM* vm, int nargs, LVal* args, LVal* out, int max_out) {
    (void)vm;
    if (max_out < 1 || nargs < 2 || args[0].type != LVAL_TABLE) return 0;
    int64_t idx = (args[1].is_int ? args[1].u.i : (int64_t)args[1].u.n) + 1;
    LVal v = ltab_get(args[0].u.t, lval_int(idx));
    if (v.type == LVAL_NIL) return 0;
    out[0] = lval_int(idx);
    if (max_out >= 2) { out[1] = v; return 2; }
    return 1;
}

static int cf_pairs(LuaVM* vm, int nargs, LVal* args, LVal* out, int max_out) {
    (void)vm;
    if (max_out < 1) return 0;
    out[0].type = LVAL_CFUNC; out[0].is_int = 0; out[0].u.cf = cf_next;
    if (max_out >= 2) out[1] = nargs >= 1 ? args[0] : lval_nil();
    if (max_out >= 3) out[2] = lval_nil();
    return max_out < 3 ? max_out : 3;
}

static int cf_ipairs(LuaVM* vm, int nargs, LVal* args, LVal* out, int max_out) {
    (void)vm;
    if (max_out < 1) return 0;
    out[0].type = LVAL_CFUNC; out[0].is_int = 0; out[0].u.cf = cf_ipairs_iter;
    if (max_out >= 2) out[1] = nargs >= 1 ? args[0] : lval_nil();
    if (max_out >= 3) out[2] = lval_int(0);
    return max_out < 3 ? max_out : 3;
}

static int cf_setmt(LuaVM* vm, int nargs, LVal* args, LVal* out, int max_out) {
    (void)vm; (void)max_out;
    if (nargs >= 1 && args[0].type == LVAL_TABLE) {
        args[0].u.t->metatable = (nargs >= 2 && args[1].type == LVAL_TABLE) ? args[1].u.t : NULL;
        out[0] = args[0];
        return 1;
    }
    return 0;
}

static int cf_dofile(LuaVM* vm, int nargs, LVal* args, LVal* out, int max_out) {
    if (nargs < 1 || args[0].type != LVAL_STR) { lua_set_error(vm, "dofile requires path"); return 0; }
    FILE* f = fopen(args[0].u.s->data, "rb");
    if (!f) { lua_set_error(vm, "cannot open file in dofile"); return 0; }
    fseek(f, 0, SEEK_END); long sz = ftell(f); fseek(f, 0, SEEK_SET);
    char* src = (char*)malloc((size_t)(sz > 0 ? sz : 1) + 1);
    size_t nr = sz > 0 ? fread(src, 1, (size_t)sz, f) : 0;
    src[nr] = '\0'; fclose(f);
    lua_dostring(vm, src, args[0].u.s->data);
    free(src);
    int c = vm->nret < max_out ? vm->nret : max_out;
    for (int i = 0; i < c; i++) out[i] = vm->ret_vals[i];
    return c;
}

/* io library */
static int cf_io_open(LuaVM* vm, int nargs, LVal* args, LVal* out, int max_out) {
    if (max_out < 1) return 0;
    if (nargs < 1 || args[0].type != LVAL_STR) {
        out[0] = lval_nil();
        if (max_out >= 2) { out[1] = mk_str(vm, "bad path", 8); return 2; }
        return 1;
    }
    const char* mode = (nargs >= 2 && args[1].type == LVAL_STR) ? args[1].u.s->data : "r";
    FILE* fp = fopen(args[0].u.s->data, mode);
    if (!fp) {
        out[0] = lval_nil();
        if (max_out >= 2) { out[1] = mk_str(vm, "cannot open file", 16); return 2; }
        return 1;
    }
    LFile* fh = lfile_new(vm, fp);
    if (!fh) {
        fclose(fp);
        out[0] = lval_nil();
        if (max_out >= 2) { out[1] = mk_str(vm, "out of memory", 13); return 2; }
        return 1;
    }
    LVal v; v.type = LVAL_FILE; v.is_int = 0; v.u.fh = fh;
    out[0] = v; return 1;
}

static int cf_file_write(LuaVM* vm, int nargs, LVal* args, LVal* out, int max_out) {
    (void)vm;
    if (nargs < 1 || args[0].type != LVAL_FILE || !args[0].u.fh || args[0].u.fh->closed || !args[0].u.fh->fp) return 0;
    FILE* fp = args[0].u.fh->fp;
    for (int i = 1; i < nargs; i++) {
        if (args[i].type == LVAL_STR) fwrite(args[i].u.s->data, 1, args[i].u.s->len, fp);
        else { char b[128]; lua_fmt_val(args[i], b, sizeof(b)); fputs(b, fp); }
    }
    if (max_out < 1) return 0;
    out[0] = args[0];
    return 1;
}

static int cf_file_read(LuaVM* vm, int nargs, LVal* args, LVal* out, int max_out) {
    if (max_out < 1) return 0;
    if (nargs < 1 || args[0].type != LVAL_FILE || !args[0].u.fh || args[0].u.fh->closed || !args[0].u.fh->fp) {
        out[0] = lval_nil(); return 1;
    }
    FILE* fp = args[0].u.fh->fp;
    const char* fmt = (nargs >= 2 && args[1].type == LVAL_STR) ? args[1].u.s->data : "*l";
    if (nargs >= 2 && args[1].type == LVAL_NUM) {
        int64_t cnt = args[1].is_int ? args[1].u.i : (int64_t)args[1].u.n;
        if (cnt < 0 || cnt > 16 * 1024 * 1024) { out[0] = lval_nil(); return 1; }
        if (cnt == 0) { out[0] = mk_str(vm, "", 0); return 1; }
        size_t n = (size_t)cnt;
        char* buf = (char*)malloc(n + 1);
        if (!buf) { out[0] = lval_nil(); return 1; }
        size_t got = fread(buf, 1, n, fp);
        out[0] = got == 0 ? lval_nil() : mk_str(vm, buf, got);
        free(buf);
        return 1;
    }
    if (strcmp(fmt, "*a") == 0 || strcmp(fmt, "a") == 0 || strcmp(fmt, "*all") == 0) {
        size_t cap = 512, len = 0;
        char* buf = (char*)malloc(cap);
        size_t got;
        while ((got = fread(buf + len, 1, cap - len - 1, fp)) > 0) {
            len += got;
            if (len + 128 >= cap) { cap *= 2; buf = (char*)realloc(buf, cap); }
        }
        out[0] = mk_str(vm, buf, len);
        free(buf);
        return 1;
    }
    char line[1024];
    if (!fgets(line, sizeof(line), fp)) { out[0] = lval_nil(); return 1; }
    size_t l = strlen(line);
    if (l > 0 && line[l - 1] == '\n') l--;
    if (l > 0 && line[l - 1] == '\r') l--;
    out[0] = mk_str(vm, line, l);
    return 1;
}

static int cf_file_close(LuaVM* vm, int nargs, LVal* args, LVal* out, int max_out) {
    (void)vm;
    if (nargs >= 1 && args[0].type == LVAL_FILE && args[0].u.fh && !args[0].u.fh->closed && args[0].u.fh->fp) {
        fclose(args[0].u.fh->fp);
        args[0].u.fh->closed = 1;
        if (max_out < 1) return 0;
        out[0] = lval_bool(1);
        return 1;
    }
    if (max_out < 1) return 0;
    out[0] = lval_bool(0); return 1;
}
static int cf_io_close(LuaVM* vm, int nargs, LVal* args, LVal* out, int max_out) { return cf_file_close(vm, nargs, args, out, max_out); }
static int cf_io_write(LuaVM* vm, int nargs, LVal* args, LVal* out, int max_out) {
    (void)vm; (void)out; (void)max_out;
    for (int i = 0; i < nargs; i++) {
        if (args[i].type == LVAL_STR) fwrite(args[i].u.s->data, 1, args[i].u.s->len, stdout);
        else { char b[128]; lua_fmt_val(args[i], b, sizeof(b)); fputs(b, stdout); }
    }
    return 0;
}
static int cf_os_clock(LuaVM* vm, int nargs, LVal* args, LVal* out, int max_out) {
    (void)vm; (void)nargs; (void)args; (void)max_out;
    struct timespec ts; clock_gettime(CLOCK_MONOTONIC, &ts);
    out[0] = lval_num((double)ts.tv_sec + (double)ts.tv_nsec * 1e-9); return 1;
}
static int cf_os_time(LuaVM* vm, int nargs, LVal* args, LVal* out, int max_out) {
    (void)vm; (void)nargs; (void)args; (void)max_out;
    struct timespec ts; clock_gettime(CLOCK_REALTIME, &ts);
    out[0] = lval_int((int64_t)ts.tv_sec); return 1;
}
static int cf_os_exit(LuaVM* vm, int nargs, LVal* args, LVal* out, int max_out) {
    (void)vm; (void)out; (void)max_out;
    int code = nargs >= 1 ? ((args[0].type == LVAL_BOOL) ? (args[0].u.b ? 0 : 1) : (int)(args[0].is_int ? args[0].u.i : args[0].u.n)) : 0;
    exit(code); return 0;
}
static int cf_os_remove(LuaVM* vm, int nargs, LVal* args, LVal* out, int max_out) {
    (void)vm; (void)max_out;
    out[0] = (nargs >= 1 && args[0].type == LVAL_STR && unlink(args[0].u.s->data) == 0) ? lval_bool(1) : lval_nil();
    return 1;
}

static int cf_os_execute(LuaVM* vm, int nargs, LVal* args, LVal* out, int max_out) {
    if (nargs < 1 || args[0].type != LVAL_STR) {
        if (max_out < 1) return 0;
        out[0] = lval_bool(1); return 1;
    }
    pid_t pid = fork();
    if (pid < 0) {
        if (max_out < 1) return 0;
        out[0] = lval_nil();
        if (max_out >= 2) out[1] = mk_str(vm, "fork", 4);
        if (max_out >= 3) out[2] = lval_int(-1);
        return max_out < 3 ? max_out : 3;
    }
    if (pid == 0) {
        const char* cmd = args[0].u.s->data;
        char* envp[] = {NULL};
        if (strcmp(cmd, "gui") == 0 || strcmp(cmd, "desktop") == 0) {
            char* g_argv[] = {"/bin/desktop_ui.elf", NULL};
            _exit(execve("/bin/desktop_ui.elf", g_argv, envp) >= 0 ? 0 : 127);
        }
        char* sh_argv[] = {"/bin/busybox", "sh", "-c", (char*)cmd, NULL};
        _exit(execve("/bin/busybox", sh_argv, envp) >= 0 ? 0 : 127);
    }
    int st = 0;
    waitpid(pid, &st, 0);
    int code = (st >> 8) & 0xFF;
    if (max_out < 1) return 0;
    out[0] = lval_bool(code == 0);
    if (max_out >= 2) out[1] = mk_str(vm, "exit", 4);
    if (max_out >= 3) out[2] = lval_int(code);
    return max_out < 3 ? max_out : 3;
}

/* string library */
static int cf_str_len(LuaVM* vm, int nargs, LVal* args, LVal* out, int max_out) {
    (void)vm; (void)max_out;
    out[0] = (nargs >= 1 && args[0].type == LVAL_STR) ? lval_int((int64_t)args[0].u.s->len) : lval_int(0);
    return 1;
}

static int cf_str_sub(LuaVM* vm, int nargs, LVal* args, LVal* out, int max_out) {
    (void)max_out;
    if (nargs < 2 || args[0].type != LVAL_STR) { out[0] = mk_str(vm, "", 0); return 1; }
    int len = (int)args[0].u.s->len;
    int i = (int)(args[1].is_int ? args[1].u.i : args[1].u.n);
    int j = (nargs >= 3) ? (int)(args[2].is_int ? args[2].u.i : args[2].u.n) : len;
    if (i < 0) i = len + i + 1;
    if (j < 0) j = len + j + 1;
    if (i < 1) i = 1;
    if (j > len) j = len;
    if (i > j) { out[0] = mk_str(vm, "", 0); return 1; }
    out[0] = mk_str(vm, args[0].u.s->data + i - 1, (size_t)(j - i + 1));
    return 1;
}

static int cf_str_upper(LuaVM* vm, int nargs, LVal* args, LVal* out, int max_out) {
    (void)max_out;
    if (nargs < 1 || args[0].type != LVAL_STR) return 0;
    size_t n = args[0].u.s->len; char* b = (char*)malloc(n + 1);
    for (size_t k = 0; k < n; k++) b[k] = (char)toupper((unsigned char)args[0].u.s->data[k]);
    out[0] = mk_str(vm, b, n); free(b); return 1;
}

static int cf_str_lower(LuaVM* vm, int nargs, LVal* args, LVal* out, int max_out) {
    (void)max_out;
    if (nargs < 1 || args[0].type != LVAL_STR) return 0;
    size_t n = args[0].u.s->len; char* b = (char*)malloc(n + 1);
    for (size_t k = 0; k < n; k++) b[k] = (char)tolower((unsigned char)args[0].u.s->data[k]);
    out[0] = mk_str(vm, b, n); free(b); return 1;
}

static int cf_str_rep(LuaVM* vm, int nargs, LVal* args, LVal* out, int max_out) {
    (void)max_out;
    if (nargs < 2 || args[0].type != LVAL_STR) { out[0] = mk_str(vm, "", 0); return 1; }
    int rep = (int)(args[1].is_int ? args[1].u.i : args[1].u.n);
    if (rep <= 0) { out[0] = mk_str(vm, "", 0); return 1; }
    size_t sl = args[0].u.s->len; char* b = (char*)malloc(sl * (size_t)rep + 1);
    if (!b) { lua_set_error(vm, "out of memory"); out[0] = lval_nil(); return 1; }
    for (int k = 0; k < rep; k++) memcpy(b + k * sl, args[0].u.s->data, sl);
    out[0] = mk_str(vm, b, sl * (size_t)rep); free(b); return 1;
}

static int cf_str_find(LuaVM* vm, int nargs, LVal* args, LVal* out, int max_out) {
    (void)vm;
    if (max_out < 1) return 0;
    if (nargs < 2 || args[0].type != LVAL_STR || args[1].type != LVAL_STR) { out[0] = lval_nil(); return 1; }
    const char* s = args[0].u.s->data; const char* p = args[1].u.s->data;
    int init = (nargs >= 3) ? (int)(args[2].is_int ? args[2].u.i : args[2].u.n) : 1;
    if (init < 1) init = 1;
    if ((size_t)(init - 1) > args[0].u.s->len) { out[0] = lval_nil(); return 1; }
    const char* hit = strstr(s + init - 1, p);
    if (!hit) { out[0] = lval_nil(); return 1; }
    out[0] = lval_int((int64_t)(hit - s + 1));
    if (max_out >= 2) { out[1] = lval_int((int64_t)(hit - s + (int)strlen(p))); return 2; }
    return 1;
}

static int cf_str_format(LuaVM* vm, int nargs, LVal* args, LVal* out, int max_out) {
    (void)max_out;
    if (nargs < 1 || args[0].type != LVAL_STR) return 0;
    const char* fmt = args[0].u.s->data;
    size_t cap = 256, len = 0; char* buf = (char*)malloc(cap);
    int ai = 1;
    while (*fmt) {
        if (*fmt != '%') {
            if (len + 2 >= cap) { cap *= 2; buf = (char*)realloc(buf, cap); }
            buf[len++] = *fmt++; continue;
        }
        fmt++;
        if (*fmt == '%') { if (len + 2 >= cap) { cap *= 2; buf = (char*)realloc(buf, cap); } buf[len++] = *fmt++; continue; }
        char spec[32]; size_t sp = 0; spec[sp++] = '%';
        while (*fmt && !isalpha((unsigned char)*fmt) && sp < 28) spec[sp++] = *fmt++;
        char conv = *fmt ? *fmt++ : 's';
        spec[sp++] = conv; spec[sp] = '\0';
        char tmp[256] = ""; LVal a = ai < nargs ? args[ai++] : lval_nil();
        if (conv == 's') { char s2[256]; lua_fmt_val(a, s2, sizeof(s2)); snprintf(tmp, sizeof(tmp), spec, s2); }
        else if (conv == 'd' || conv == 'i' || conv == 'x' || conv == 'X' || conv == 'u') {
            long long iv = (a.type == LVAL_NUM) ? (a.is_int ? a.u.i : (long long)a.u.n) : 0;
            char dspec[36];
            snprintf(dspec, sizeof(dspec), "%.*sll%c", (int)(sp - 1), spec, conv);
            snprintf(tmp, sizeof(tmp), dspec, iv);
        } else if (conv == 'f' || conv == 'g' || conv == 'e') {
            int ok = 0; double dv = lval_as_num(a, &ok);
            snprintf(tmp, sizeof(tmp), spec, dv);
        } else if (conv == 'c') {
            int cv = (int)(a.is_int ? a.u.i : a.u.n);
            snprintf(tmp, sizeof(tmp), "%c", cv);
        }
        size_t tl = strlen(tmp);
        while (len + tl + 2 >= cap) { cap *= 2; buf = (char*)realloc(buf, cap); }
        memcpy(buf + len, tmp, tl); len += tl;
    }
    out[0] = mk_str(vm, buf, len); free(buf); return 1;
}

/* table library */
static int cf_tab_insert(LuaVM* vm, int nargs, LVal* args, LVal* out, int max_out) {
    (void)out; (void)max_out;
    if (nargs < 2 || args[0].type != LVAL_TABLE) return 0;
    LTable* t = args[0].u.t;
    int len = ltab_len(t);
    int pos = (nargs >= 3) ? (int)(args[1].is_int ? args[1].u.i : args[1].u.n) : (len + 1);
    LVal val = (nargs >= 3) ? args[2] : args[1];
    for (int i = len; i >= pos; i--) ltab_set(vm, t, lval_int(i + 1), ltab_get(t, lval_int(i)));
    ltab_set(vm, t, lval_int(pos), val);
    return 0;
}

static int cf_tab_remove(LuaVM* vm, int nargs, LVal* args, LVal* out, int max_out) {
    (void)max_out;
    if (nargs < 1 || args[0].type != LVAL_TABLE) return 0;
    LTable* t = args[0].u.t;
    int len = ltab_len(t);
    int pos = (nargs >= 2) ? (int)(args[1].is_int ? args[1].u.i : args[1].u.n) : len;
    if (pos < 1 || pos > len) { out[0] = lval_nil(); return 1; }
    out[0] = ltab_get(t, lval_int(pos));
    for (int i = pos; i < len; i++) ltab_set(vm, t, lval_int(i), ltab_get(t, lval_int(i + 1)));
    ltab_set(vm, t, lval_int(len), lval_nil());
    return 1;
}

static int cf_tab_concat(LuaVM* vm, int nargs, LVal* args, LVal* out, int max_out) {
    (void)max_out;
    if (nargs < 1 || args[0].type != LVAL_TABLE) { out[0] = mk_str(vm, "", 0); return 1; }
    const char* sep = (nargs >= 2 && args[1].type == LVAL_STR) ? args[1].u.s->data : "";
    size_t slen = strlen(sep); int len = ltab_len(args[0].u.t);
    size_t cap = 256, bl = 0; char* buf = (char*)malloc(cap);
    for (int i = 1; i <= len; i++) {
        if (i > 1 && slen) {
            while (bl + slen + 2 >= cap) { cap *= 2; buf = (char*)realloc(buf, cap); }
            memcpy(buf + bl, sep, slen); bl += slen;
        }
        char tmp[128]; LVal v = ltab_get(args[0].u.t, lval_int(i));
        lua_fmt_val(v, tmp, sizeof(tmp));
        size_t tl = strlen(tmp);
        while (bl + tl + 2 >= cap) { cap *= 2; buf = (char*)realloc(buf, cap); }
        memcpy(buf + bl, tmp, tl); bl += tl;
    }
    out[0] = mk_str(vm, buf, bl); free(buf); return 1;
}

static int cf_math_abs(LuaVM* vm, int nargs, LVal* args, LVal* out, int max_out) {
    (void)vm; (void)max_out;
    if (nargs < 1 || args[0].type != LVAL_NUM) return 0;
    out[0] = args[0].is_int ? lval_int(args[0].u.i < 0 ? -args[0].u.i : args[0].u.i) : lval_num(fabs(args[0].u.n));
    return 1;
}
static int cf_math_floor(LuaVM* vm, int nargs, LVal* args, LVal* out, int max_out) {
    (void)vm; (void)max_out; int ok = 0; double d = nargs >= 1 ? lval_as_num(args[0], &ok) : 0.0;
    out[0] = lval_int((int64_t)floor(d)); return 1;
}
static int cf_math_ceil(LuaVM* vm, int nargs, LVal* args, LVal* out, int max_out) {
    (void)vm; (void)max_out; int ok = 0; double d = nargs >= 1 ? lval_as_num(args[0], &ok) : 0.0;
    out[0] = lval_int((int64_t)ceil(d)); return 1;
}
static int cf_math_sqrt(LuaVM* vm, int nargs, LVal* args, LVal* out, int max_out) {
    (void)vm; (void)max_out; int ok = 0; double d = nargs >= 1 ? lval_as_num(args[0], &ok) : 0.0;
    out[0] = lval_num(sqrt(d)); return 1;
}
static int cf_math_min(LuaVM* vm, int nargs, LVal* args, LVal* out, int max_out) {
    (void)vm; (void)max_out; if (nargs < 1) return 0;
    LVal best = args[0]; int ok = 0; double bv = lval_as_num(best, &ok);
    for (int i = 1; i < nargs; i++) { double v = lval_as_num(args[i], &ok); if (v < bv) { bv = v; best = args[i]; } }
    out[0] = best; return 1;
}
static int cf_math_max(LuaVM* vm, int nargs, LVal* args, LVal* out, int max_out) {
    (void)vm; (void)max_out; if (nargs < 1) return 0;
    LVal best = args[0]; int ok = 0; double bv = lval_as_num(best, &ok);
    for (int i = 1; i < nargs; i++) { double v = lval_as_num(args[i], &ok); if (v > bv) { bv = v; best = args[i]; } }
    out[0] = best; return 1;
}

void lua_openlibs(LuaVM* vm) {
    LTable* g = vm->globals->vars;
    set_global_cfunc(vm, g, "print", cf_print);
    set_global_cfunc(vm, g, "type", cf_type);
    set_global_cfunc(vm, g, "tostring", cf_tostring);
    set_global_cfunc(vm, g, "tonumber", cf_tonumber);
    set_global_cfunc(vm, g, "assert", cf_assert);
    set_global_cfunc(vm, g, "error", cf_error);
    set_global_cfunc(vm, g, "pcall", cf_pcall);
    set_global_cfunc(vm, g, "next", cf_next);
    set_global_cfunc(vm, g, "pairs", cf_pairs);
    set_global_cfunc(vm, g, "ipairs", cf_ipairs);
    set_global_cfunc(vm, g, "setmetatable", cf_setmt);
    set_global_cfunc(vm, g, "dofile", cf_dofile);
    ltab_set(vm, g, mk_str(vm, "_VERSION", 8), mk_str(vm, "Lua 5.4", 7));

    LTable* io = ltab_new(vm);
    set_global_cfunc(vm, io, "open", cf_io_open);
    set_global_cfunc(vm, io, "close", cf_io_close);
    set_global_cfunc(vm, io, "write", cf_io_write);
    LVal iov; iov.type = LVAL_TABLE; iov.is_int = 0; iov.u.t = io;
    ltab_set(vm, g, mk_str(vm, "io", 2), iov);

    LTable* fmt = ltab_new(vm);
    set_global_cfunc(vm, fmt, "read", cf_file_read);
    set_global_cfunc(vm, fmt, "write", cf_file_write);
    set_global_cfunc(vm, fmt, "close", cf_file_close);
    LVal fmtv; fmtv.type = LVAL_TABLE; fmtv.is_int = 0; fmtv.u.t = fmt;
    ltab_set(vm, g, mk_str(vm, "__file_mt", 9), fmtv);

    LTable* os = ltab_new(vm);
    set_global_cfunc(vm, os, "clock", cf_os_clock);
    set_global_cfunc(vm, os, "time", cf_os_time);
    set_global_cfunc(vm, os, "exit", cf_os_exit);
    set_global_cfunc(vm, os, "remove", cf_os_remove);
    set_global_cfunc(vm, os, "execute", cf_os_execute);
    LVal osv; osv.type = LVAL_TABLE; osv.is_int = 0; osv.u.t = os;
    ltab_set(vm, g, mk_str(vm, "os", 2), osv);

    LTable* str = ltab_new(vm);
    set_global_cfunc(vm, str, "len", cf_str_len);
    set_global_cfunc(vm, str, "sub", cf_str_sub);
    set_global_cfunc(vm, str, "upper", cf_str_upper);
    set_global_cfunc(vm, str, "lower", cf_str_lower);
    set_global_cfunc(vm, str, "rep", cf_str_rep);
    set_global_cfunc(vm, str, "find", cf_str_find);
    set_global_cfunc(vm, str, "format", cf_str_format);
    vm->string_mt = str;
    LVal strv; strv.type = LVAL_TABLE; strv.is_int = 0; strv.u.t = str;
    ltab_set(vm, g, mk_str(vm, "string", 6), strv);

    LTable* tab = ltab_new(vm);
    set_global_cfunc(vm, tab, "insert", cf_tab_insert);
    set_global_cfunc(vm, tab, "remove", cf_tab_remove);
    set_global_cfunc(vm, tab, "concat", cf_tab_concat);
    LVal tabv; tabv.type = LVAL_TABLE; tabv.is_int = 0; tabv.u.t = tab;
    ltab_set(vm, g, mk_str(vm, "table", 5), tabv);

    LTable* mth = ltab_new(vm);
    set_global_cfunc(vm, mth, "abs", cf_math_abs);
    set_global_cfunc(vm, mth, "floor", cf_math_floor);
    set_global_cfunc(vm, mth, "ceil", cf_math_ceil);
    set_global_cfunc(vm, mth, "sqrt", cf_math_sqrt);
    set_global_cfunc(vm, mth, "min", cf_math_min);
    set_global_cfunc(vm, mth, "max", cf_math_max);
    ltab_set(vm, mth, mk_str(vm, "pi", 2), lval_num(3.14159265358979323846));
    ltab_set(vm, mth, mk_str(vm, "huge", 4), lval_num(INFINITY));
    LVal mthv; mthv.type = LVAL_TABLE; mthv.is_int = 0; mthv.u.t = mth;
    ltab_set(vm, g, mk_str(vm, "math", 4), mthv);
}
