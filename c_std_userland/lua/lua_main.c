#include "lua.h"
#include <stdlib.h>
#include <string.h>

LuaVM* lua_vm_new(void) {
    LuaVM* vm = (LuaVM*)calloc(1, sizeof(LuaVM));
    vm->gc_thresh = 256;
    vm->globals = lenv_new(vm, NULL);
    vm->cur_env = vm->globals;
    return vm;
}

void lua_vm_free(LuaVM* vm) {
    if (!vm) return;
    GcObj* o = vm->gc_head;
    while (o) {
        GcObj* next = o->next;
        if (o->kind == GC_TABLE) free(((LTable*)o)->entries);
        else if (o->kind == GC_FILE) {
            LFile* lf = (LFile*)o;
            if (lf->fp && !lf->closed && lf->fp != stdin && lf->fp != stdout && lf->fp != stderr) {
                fclose(lf->fp);
                lf->closed = 1;
            }
        }
        free(o);
        o = next;
    }
    for (int i = 0; i < vm->nchunks; i++) lua_free_ast(vm->chunks[i]);
    free(vm->chunks);
    free(vm);
}

void lua_set_error(LuaVM* vm, const char* msg) {
    if (!vm->err_flag) {
        vm->err_flag = 1;
        snprintf(vm->err_msg, sizeof(vm->err_msg), "%s", msg ? msg : "error");
    }
}

int lua_dostring(LuaVM* vm, const char* src, const char* name) {
    char err[256] = "";
    int ntoks = 0;
    Token* toks = lua_tokenize(src, &ntoks, err, sizeof(err));
    if (!toks) {
        lua_set_error(vm, err);
        fprintf(stderr, "lua: %s: %s\n", name, err);
        return 1;
    }
    LAst* ast = lua_parse(toks, ntoks, err, sizeof(err));
    lua_free_tokens(toks, ntoks);
    if (!ast) {
        lua_set_error(vm, err);
        fprintf(stderr, "lua: %s: %s\n", name, err);
        return 1;
    }
    if (vm->nchunks + 1 >= vm->chunk_cap) {
        vm->chunk_cap = vm->chunk_cap ? vm->chunk_cap * 2 : 8;
        vm->chunks = (LAst**)realloc(vm->chunks, (size_t)vm->chunk_cap * sizeof(LAst*));
    }
    vm->chunks[vm->nchunks++] = ast;

    vm->returning = 0; vm->breaking = 0; vm->err_flag = 0; vm->nret = 0;
    lua_exec_ast(vm, ast);
    if (vm->err_flag) {
        fprintf(stderr, "lua: runtime error: %s\n", vm->err_msg);
        return 1;
    }
    return 0;
}

static int run_repl(LuaVM* vm) {
    printf("Lua 5.4.7 (FipSOs v4.8)  Native C11 Interpreter\n");
    fflush(stdout);
    char line[1024];
    for (;;) {
        printf("> ");
        fflush(stdout);
        if (!fgets(line, sizeof(line), stdin)) {
            putchar('\n');
            break;
        }
        size_t len = strlen(line);
        while (len > 0 && (line[len - 1] == '\n' || line[len - 1] == '\r')) line[--len] = '\0';
        if (len == 0) continue;
        if (strcmp(line, ".exit") == 0 || strcmp(line, "exit") == 0 || strcmp(line, "quit") == 0) break;
        if (line[0] == '=') {
            char expr_buf[1100];
            snprintf(expr_buf, sizeof(expr_buf), "return (%s)", line + 1);
            if (lua_dostring(vm, expr_buf, "=stdin") == 0 && vm->nret > 0) {
                for (int i = 0; i < vm->nret; i++) {
                    char b[128]; lua_fmt_val(vm->ret_vals[i], b, sizeof(b));
                    printf("%s%s", i > 0 ? "\t" : "", b);
                }
                putchar('\n');
            }
        } else {
            lua_dostring(vm, line, "=stdin");
        }
    }
    return 0;
}

int main(int argc, char** argv) {
    LuaVM* vm = lua_vm_new();
    lua_openlibs(vm);

    int rc = 0;
    if (argc < 2) {
        rc = run_repl(vm);
    } else if (strcmp(argv[1], "-e") == 0) {
        if (argc < 3) {
            fprintf(stderr, "lua: '-e' needs argument\n");
            rc = 1;
        } else {
            rc = lua_dostring(vm, argv[2], "=(command line)");
        }
    } else if (strcmp(argv[1], "-i") == 0) {
        rc = run_repl(vm);
    } else {
        const char* path = argv[1];
        LTable* argt = ltab_new(vm);
        for (int i = 1; i < argc; i++) {
            LVal sv; sv.type = LVAL_STR; sv.is_int = 0; sv.u.s = lstr_new(vm, argv[i], strlen(argv[i]));
            ltab_set(vm, argt, lval_int(i - 1), sv);
        }
        LStr* ak = lstr_new(vm, "arg", 3);
        LVal akv; akv.type = LVAL_STR; akv.is_int = 0; akv.u.s = ak;
        LVal av; av.type = LVAL_TABLE; av.is_int = 0; av.u.t = argt;
        ltab_set(vm, vm->globals->vars, akv, av);

        FILE* f = fopen(path, "rb");
        if (!f) {
            fprintf(stderr, "lua: cannot open %s\n", path);
            lua_vm_free(vm);
            return 1;
        }
        fseek(f, 0, SEEK_END); long sz = ftell(f); fseek(f, 0, SEEK_SET);
        size_t cap = (size_t)(sz > 0 ? sz : 1);
        char* src = (char*)malloc(cap + 1);
        size_t nr = sz > 0 ? fread(src, 1, cap, f) : 0;
        src[nr] = '\0';
        fclose(f);
        rc = lua_dostring(vm, src, path);
        free(src);
    }

    lua_vm_free(vm);
    return rc;
}
