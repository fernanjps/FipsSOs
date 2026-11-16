#include "lua.h"
#include <math.h>
#include <stdlib.h>
#include <string.h>

static uint32_t fnv1a(const char* s, size_t len) {
    uint32_t h = 2166136261u;
    for (size_t i = 0; i < len; i++) {
        h ^= (uint8_t)s[i];
        h *= 16777619u;
    }
    return h;
}

static uint32_t hash_val(LVal k) {
    switch (k.type) {
    case LVAL_BOOL: return (uint32_t)(k.u.b ? 3 : 2);
    case LVAL_NUM: {
        double d = k.is_int ? (double)k.u.i : k.u.n;
        uint64_t bits = 0;
        if (d == 0.0) d = 0.0;
        memcpy(&bits, &d, sizeof(bits));
        return (uint32_t)(bits ^ (bits >> 32));
    }
    case LVAL_STR: return k.u.s->hash;
    default: return (uint32_t)((uintptr_t)k.u.t >> 3);
    }
}

static GcObj* gc_alloc(LuaVM* vm, GcKind kind, size_t sz) {
    if (vm->obj_count >= vm->gc_thresh) {
        lua_gc_run(vm);
        if (vm->obj_count >= vm->gc_thresh) vm->gc_thresh *= 2;
    }
    GcObj* o = (GcObj*)calloc(1, sz);
    if (!o) return NULL;
    o->kind = (uint8_t)kind;
    o->marked = 0;
    o->next = vm->gc_head;
    vm->gc_head = o;
    vm->obj_count++;
    return o;
}

LVal lval_nil(void) { LVal v; memset(&v, 0, sizeof(v)); v.type = LVAL_NIL; return v; }
LVal lval_bool(int b) { LVal v; memset(&v, 0, sizeof(v)); v.type = LVAL_BOOL; v.u.b = b ? 1 : 0; return v; }
LVal lval_int(int64_t i) { LVal v; memset(&v, 0, sizeof(v)); v.type = LVAL_NUM; v.is_int = 1; v.u.i = i; return v; }
LVal lval_num(double d) { LVal v; memset(&v, 0, sizeof(v)); v.type = LVAL_NUM; v.is_int = 0; v.u.n = d; return v; }

double lval_as_num(LVal v, int* ok) {
    if (v.type == LVAL_NUM) { *ok = 1; return v.is_int ? (double)v.u.i : v.u.n; }
    if (v.type == LVAL_STR) {
        char* end = NULL;
        double d = strtod(v.u.s->data, &end);
        if (end && end != v.u.s->data && *end == '\0') { *ok = 1; return d; }
    }
    *ok = 0;
    return 0.0;
}

int lval_truthy(LVal v) { return !(v.type == LVAL_NIL || (v.type == LVAL_BOOL && !v.u.b)); }

int lval_eq(LVal a, LVal b) {
    if (a.type != b.type) return 0;
    switch (a.type) {
    case LVAL_NIL: return 1;
    case LVAL_BOOL: return a.u.b == b.u.b;
    case LVAL_NUM: {
        if (a.is_int && b.is_int) return a.u.i == b.u.i;
        double da = a.is_int ? (double)a.u.i : a.u.n;
        double db = b.is_int ? (double)b.u.i : b.u.n;
        return da == db;
    }
    case LVAL_STR: return a.u.s->len == b.u.s->len && memcmp(a.u.s->data, b.u.s->data, a.u.s->len) == 0;
    case LVAL_TABLE: return a.u.t == b.u.t;
    case LVAL_FUNC: return a.u.f == b.u.f;
    case LVAL_CFUNC: return a.u.cf == b.u.cf;
    case LVAL_FILE: return a.u.fh == b.u.fh;
    default: return 0;
    }
}

LStr* lstr_new(LuaVM* vm, const char* s, size_t len) {
    LStr* obj = (LStr*)gc_alloc(vm, GC_STR, sizeof(LStr) + len + 1);
    if (!obj) return NULL;
    obj->len = len;
    obj->hash = fnv1a(s ? s : "", len);
    if (len && s) memcpy(obj->data, s, len);
    obj->data[len] = '\0';
    return obj;
}

LTable* ltab_new(LuaVM* vm) {
    LTable* t = (LTable*)gc_alloc(vm, GC_TABLE, sizeof(LTable));
    if (!t) return NULL;
    t->cap = 16;
    t->count = 0;
    t->entries = (LTabEntry*)calloc(t->cap, sizeof(LTabEntry));
    return t;
}

LEnv* lenv_new(LuaVM* vm, LEnv* parent) {
    int base = vm->nroots;
    LVal pv = lval_nil();
    if (parent && parent->vars) {
        pv.type = LVAL_TABLE; pv.u.t = parent->vars;
        lua_gc_push_root(vm, pv);
    }
    LTable* vars = ltab_new(vm);
    LVal tv; tv.type = LVAL_TABLE; tv.is_int = 0; tv.u.t = vars;
    lua_gc_push_root(vm, tv);
    LEnv* e = (LEnv*)gc_alloc(vm, GC_ENV, sizeof(LEnv));
    vm->nroots = base;
    if (!e) return NULL;
    e->parent = parent;
    e->vars = vars;
    return e;
}

LFunc* lfunc_new(LuaVM* vm) { return (LFunc*)gc_alloc(vm, GC_FUNC, sizeof(LFunc)); }

LFile* lfile_new(LuaVM* vm, FILE* fp) {
    LFile* f = (LFile*)gc_alloc(vm, GC_FILE, sizeof(LFile));
    if (!f) return NULL;
    f->fp = fp;
    f->closed = 0;
    return f;
}

static void ltab_grow(LTable* t) {
    size_t ncap = t->cap ? t->cap * 2 : 16;
    LTabEntry* nent = (LTabEntry*)calloc(ncap, sizeof(LTabEntry));
    for (size_t i = 0; i < t->cap; i++) {
        if (t->entries[i].used && t->entries[i].val.type != LVAL_NIL) {
            size_t idx = hash_val(t->entries[i].key) & (ncap - 1);
            while (nent[idx].used) idx = (idx + 1) & (ncap - 1);
            nent[idx] = t->entries[i];
        }
    }
    free(t->entries);
    t->entries = nent;
    t->cap = ncap;
    size_t cnt = 0;
    for (size_t i = 0; i < ncap; i++) if (nent[i].used) cnt++;
    t->count = cnt;
}

LVal ltab_get(LTable* t, LVal key) {
    if (!t || !t->cap || key.type == LVAL_NIL) return lval_nil();
    size_t idx = hash_val(key) & (t->cap - 1);
    for (size_t i = 0; i < t->cap; i++) {
        LTabEntry* e = &t->entries[idx];
        if (!e->used) return lval_nil();
        if (lval_eq(e->key, key)) return e->val;
        idx = (idx + 1) & (t->cap - 1);
    }
    return lval_nil();
}

void ltab_set(LuaVM* vm, LTable* t, LVal key, LVal val) {
    (void)vm;
    if (!t || key.type == LVAL_NIL) return;
    if ((t->count + 1) * 2 >= t->cap) ltab_grow(t);
    size_t idx = hash_val(key) & (t->cap - 1);
    for (size_t i = 0; i < t->cap; i++) {
        LTabEntry* e = &t->entries[idx];
        if (!e->used) {
            if (val.type == LVAL_NIL) return;
            e->used = 1;
            e->key = key;
            e->val = val;
            t->count++;
            return;
        }
        if (lval_eq(e->key, key)) {
            e->val = val;
            return;
        }
        idx = (idx + 1) & (t->cap - 1);
    }
}

int ltab_len(LTable* t) {
    if (!t) return 0;
    int i = 1;
    while (ltab_get(t, lval_int(i)).type != LVAL_NIL) i++;
    return i - 1;
}

int ltab_next(LTable* t, LVal key, LVal* out_k, LVal* out_v) {
    if (!t) return 0;
    size_t start = 0;
    if (key.type != LVAL_NIL) {
        size_t i = 0;
        for (; i < t->cap; i++) {
            if (t->entries[i].used && t->entries[i].val.type != LVAL_NIL && lval_eq(t->entries[i].key, key)) {
                start = i + 1;
                break;
            }
        }
        if (i == t->cap) return 0;
    }
    for (size_t i = start; i < t->cap; i++) {
        if (t->entries[i].used && t->entries[i].val.type != LVAL_NIL) {
            *out_k = t->entries[i].key;
            *out_v = t->entries[i].val;
            return 1;
        }
    }
    return 0;
}

LVal lenv_get(LEnv* e, LStr* name) {
    LVal k; k.type = LVAL_STR; k.is_int = 0; k.u.s = name;
    for (LEnv* cur = e; cur; cur = cur->parent) {
        LVal v = ltab_get(cur->vars, k);
        if (v.type != LVAL_NIL) return v;
    }
    return lval_nil();
}

void lenv_set_local(LuaVM* vm, LEnv* e, LStr* name, LVal val) {
    LVal k; k.type = LVAL_STR; k.is_int = 0; k.u.s = name;
    ltab_set(vm, e->vars, k, val);
}

void lenv_assign(LuaVM* vm, LEnv* e, LStr* name, LVal val) {
    LVal k; k.type = LVAL_STR; k.is_int = 0; k.u.s = name;
    for (LEnv* cur = e; cur; cur = cur->parent) {
        if (ltab_get(cur->vars, k).type != LVAL_NIL || !cur->parent) {
            ltab_set(vm, cur->vars, k, val);
            return;
        }
    }
}

void lua_fmt_val(LVal v, char* buf, size_t cap) {
    switch (v.type) {
    case LVAL_NIL: snprintf(buf, cap, "nil"); break;
    case LVAL_BOOL: snprintf(buf, cap, "%s", v.u.b ? "true" : "false"); break;
    case LVAL_NUM:
        if (v.is_int) {
            snprintf(buf, cap, "%lld", (long long)v.u.i);
        } else {
            double ip;
            if (modf(v.u.n, &ip) == 0.0 && fabs(v.u.n) < 1e15) {
                snprintf(buf, cap, "%.14g", v.u.n);
            } else {
                snprintf(buf, cap, "%.14g", v.u.n);
            }
        }
        break;
    case LVAL_STR: snprintf(buf, cap, "%.*s", (int)v.u.s->len, v.u.s->data); break;
    case LVAL_TABLE: snprintf(buf, cap, "table: %p", (void*)v.u.t); break;
    case LVAL_FUNC: snprintf(buf, cap, "function: %p", (void*)v.u.f); break;
    case LVAL_CFUNC: snprintf(buf, cap, "function: %p", (void*)(uintptr_t)v.u.cf); break;
    case LVAL_FILE: snprintf(buf, cap, "file (%p)", (void*)(v.u.fh ? v.u.fh->fp : NULL)); break;
    default: snprintf(buf, cap, "unknown"); break;
    }
}

static void gc_mark_obj(GcObj* o);

static void gc_mark_val(LVal v) {
    switch (v.type) {
    case LVAL_STR: gc_mark_obj(&v.u.s->gc); break;
    case LVAL_TABLE: gc_mark_obj(&v.u.t->gc); break;
    case LVAL_FUNC: gc_mark_obj(&v.u.f->gc); break;
    case LVAL_FILE: gc_mark_obj(&v.u.fh->gc); break;
    default: break;
    }
}

static void gc_mark_obj(GcObj* o) {
    if (!o || o->marked) return;
    o->marked = 1;
    if (o->kind == GC_TABLE) {
        LTable* t = (LTable*)o;
        for (size_t i = 0; i < t->cap; i++) {
            if (t->entries[i].used) {
                gc_mark_val(t->entries[i].key);
                gc_mark_val(t->entries[i].val);
            }
        }
        if (t->metatable) gc_mark_obj(&t->metatable->gc);
    } else if (o->kind == GC_ENV) {
        LEnv* e = (LEnv*)o;
        if (e->parent) gc_mark_obj(&e->parent->gc);
        if (e->vars) gc_mark_obj(&e->vars->gc);
    } else if (o->kind == GC_FUNC) {
        LFunc* f = (LFunc*)o;
        if (f->closure) gc_mark_obj(&f->closure->gc);
    }
}

void lua_gc_push_root(LuaVM* vm, LVal v) {
    if (vm->nroots < LUA_MAX_ROOTS) vm->roots[vm->nroots++] = v;
}

void lua_gc_pop_roots(LuaVM* vm, int count) {
    vm->nroots = vm->nroots >= count ? vm->nroots - count : 0;
}

void lua_gc_run(LuaVM* vm) {
    if (!vm) return;
    if (vm->globals) gc_mark_obj(&vm->globals->gc);
    if (vm->cur_env) gc_mark_obj(&vm->cur_env->gc);
    if (vm->string_mt) gc_mark_obj(&vm->string_mt->gc);
    for (int i = 0; i < vm->nroots; i++) gc_mark_val(vm->roots[i]);
    for (int i = 0; i < vm->nret; i++) gc_mark_val(vm->ret_vals[i]);
    for (int i = 0; i < vm->nvarargs; i++) gc_mark_val(vm->varargs[i]);

    GcObj** p = &vm->gc_head;
    while (*p) {
        GcObj* o = *p;
        if (o->marked) {
            o->marked = 0;
            p = &o->next;
        } else {
            *p = o->next;
            if (o->kind == GC_TABLE) free(((LTable*)o)->entries);
            else if (o->kind == GC_FILE) {
                LFile* lf = (LFile*)o;
                if (lf->fp && !lf->closed && lf->fp != stdin && lf->fp != stdout && lf->fp != stderr) {
                    fclose(lf->fp);
                    lf->closed = 1;
                }
            }
            free(o);
            vm->obj_count--;
        }
    }
}
