#ifndef FIPS_LUA_H
#define FIPS_LUA_H

#include <stddef.h>
#include <stdint.h>
#include <stdio.h>

typedef enum {
    LVAL_NIL = 0,
    LVAL_BOOL,
    LVAL_NUM,
    LVAL_STR,
    LVAL_TABLE,
    LVAL_FUNC,
    LVAL_CFUNC,
    LVAL_FILE
} LValType;

typedef enum {
    GC_STR = 1,
    GC_TABLE,
    GC_FUNC,
    GC_ENV,
    GC_FILE
} GcKind;

typedef struct GcObj {
    uint8_t kind;
    uint8_t marked;
    struct GcObj* next;
} GcObj;

typedef struct LStr {
    GcObj gc;
    size_t len;
    uint32_t hash;
    char data[];
} LStr;

typedef struct LVal LVal;
typedef struct LTable LTable;
typedef struct LFunc LFunc;
typedef struct LEnv LEnv;
typedef struct LFile LFile;
typedef struct LuaVM LuaVM;

typedef int (*LCFunc)(LuaVM* vm, int nargs, LVal* args, LVal* out, int max_out);

struct LFile {
    GcObj gc;
    FILE* fp;
    int closed;
};

struct LVal {
    uint8_t type;
    uint8_t is_int;
    union {
        int b;
        double n;
        int64_t i;
        LStr* s;
        LTable* t;
        LFunc* f;
        LCFunc cf;
        LFile* fh;
    } u;
};

typedef struct LTabEntry {
    LVal key;
    LVal val;
    uint8_t used;
} LTabEntry;

struct LTable {
    GcObj gc;
    LTabEntry* entries;
    size_t cap;
    size_t count;
    LTable* metatable;
};

struct LEnv {
    GcObj gc;
    LEnv* parent;
    LTable* vars;
};

typedef struct LAst LAst;

struct LFunc {
    GcObj gc;
    char** params;
    int nparams;
    int has_vararg;
    LAst* body;
    LEnv* closure;
    char name[64];
};

typedef enum {
    TK_EOF = 0, TK_NAME, TK_NUM, TK_STR,
    TK_AND, TK_BREAK, TK_DO, TK_ELSE, TK_ELSEIF, TK_END,
    TK_FALSE, TK_FOR, TK_FUNCTION, TK_IF, TK_IN, TK_LOCAL,
    TK_NIL, TK_NOT, TK_OR, TK_REPEAT, TK_RETURN, TK_THEN,
    TK_TRUE, TK_UNTIL, TK_WHILE,
    TK_ADD, TK_SUB, TK_MUL, TK_DIV, TK_IDIV, TK_MOD, TK_POW, TK_LEN,
    TK_EQ, TK_NE, TK_LE, TK_GE, TK_LT, TK_GT, TK_ASSIGN,
    TK_LPAREN, TK_RPAREN, TK_LBRACE, TK_RBRACE, TK_LBRACK, TK_RBRACK,
    TK_SEMI, TK_COLON, TK_COMMA, TK_DOT, TK_CONCAT, TK_DOTS
} TokKind;

typedef struct {
    TokKind kind;
    int line;
    double num;
    int64_t inum;
    int is_int;
    char* text;
    size_t len;
} Token;

typedef enum {
    AST_BLOCK = 1, AST_NIL, AST_BOOL, AST_NUM, AST_STR, AST_VAR, AST_VARARG,
    AST_TABLE, AST_INDEX, AST_UNOP, AST_BINOP, AST_CALL, AST_METHOD_CALL,
    AST_FUNC_EXPR, AST_ASSIGN, AST_LOCAL, AST_IF, AST_WHILE, AST_REPEAT,
    AST_FOR_NUM, AST_FOR_IN, AST_RETURN, AST_BREAK, AST_DO
} AstKind;

typedef struct {
    LAst* key; /* NULL for positional array field */
    LAst* val;
} AstField;

typedef struct {
    LAst* cond;
    LAst* body;
} AstBranch;

struct LAst {
    AstKind kind;
    int line;
    union {
        struct { LAst** stmts; int nstmts; } block;
        struct { int b; } bval;
        struct { double n; int64_t i; int is_int; } nval;
        struct { char* s; size_t len; } sval;
        struct { char* name; } var;
        struct { AstField* fields; int nfields; } tab;
        struct { LAst* obj; LAst* key; } idx;
        struct { TokKind op; LAst* expr; } unop;
        struct { TokKind op; LAst* left; LAst* right; } binop;
        struct { LAst* fn; LAst** args; int nargs; char* method; } call;
        struct { char** params; int nparams; int has_vararg; LAst* body; char name[64]; } func;
        struct { LAst** lhs; int nlhs; LAst** rhs; int nrhs; } assign;
        struct { char** names; int nnames; LAst** rhs; int nrhs; } local;
        struct { AstBranch* branches; int nbranches; LAst* else_body; } ifs;
        struct { LAst* cond; LAst* body; } loop;
        struct { char* var; LAst* start; LAst* stop; LAst* step; LAst* body; } fornum;
        struct { char** vars; int nvars; LAst** iters; int niters; LAst* body; } forin;
        struct { LAst** vals; int nvals; } ret;
    } u;
};

#define LUA_MAX_ROOTS 512

struct LuaVM {
    GcObj* gc_head;
    size_t obj_count;
    size_t gc_thresh;
    LEnv* globals;
    LEnv* cur_env;
    LTable* string_mt;
    LVal roots[LUA_MAX_ROOTS];
    int nroots;
    int returning;
    int breaking;
    LVal ret_vals[16];
    int nret;
    int err_flag;
    char err_msg[256];
    LVal* varargs;
    int nvarargs;
    LAst** chunks;
    int nchunks;
    int chunk_cap;
};

/* lua_val.c */
LVal lval_nil(void);
LVal lval_bool(int b);
LVal lval_int(int64_t i);
LVal lval_num(double d);
double lval_as_num(LVal v, int* ok);
int lval_truthy(LVal v);
int lval_eq(LVal a, LVal b);
LStr* lstr_new(LuaVM* vm, const char* s, size_t len);
LTable* ltab_new(LuaVM* vm);
LEnv* lenv_new(LuaVM* vm, LEnv* parent);
LFunc* lfunc_new(LuaVM* vm);
LFile* lfile_new(LuaVM* vm, FILE* fp);
LVal ltab_get(LTable* t, LVal key);
void ltab_set(LuaVM* vm, LTable* t, LVal key, LVal val);
int ltab_len(LTable* t);
int ltab_next(LTable* t, LVal key, LVal* out_k, LVal* out_v);
LVal lenv_get(LEnv* e, LStr* name);
void lenv_set_local(LuaVM* vm, LEnv* e, LStr* name, LVal val);
void lenv_assign(LuaVM* vm, LEnv* e, LStr* name, LVal val);
void lua_fmt_val(LVal v, char* buf, size_t cap);
void lua_gc_run(LuaVM* vm);
void lua_gc_push_root(LuaVM* vm, LVal v);
void lua_gc_pop_roots(LuaVM* vm, int count);

/* lua_lex.c */
Token* lua_tokenize(const char* src, int* out_count, char* err, size_t err_cap);
void lua_free_tokens(Token* toks, int count);

/* lua_parse.c */
LAst* lua_parse(const Token* toks, int ntoks, char* err, size_t err_cap);
void lua_free_ast(LAst* node);

/* lua_eval.c */
LuaVM* lua_vm_new(void);
void lua_vm_free(LuaVM* vm);
void lua_set_error(LuaVM* vm, const char* msg);
int lua_exec_ast(LuaVM* vm, LAst* block);
int lua_call_val(LuaVM* vm, LVal fn, int nargs, LVal* args, LVal* out, int max_out);
int lua_dostring(LuaVM* vm, const char* src, const char* name);

/* lua_lib.c */
void lua_openlibs(LuaVM* vm);

#endif
