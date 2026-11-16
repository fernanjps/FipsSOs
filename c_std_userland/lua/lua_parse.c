#include "lua.h"
#include <stdlib.h>
#include <string.h>

typedef struct {
    const Token* toks;
    int n;
    int pos;
    int err;
    char* emsg;
    size_t ecap;
} Parser;

static char* xstrdup(const char* s, size_t len) {
    char* p = (char*)malloc(len + 1);
    if (s && len) memcpy(p, s, len);
    p[len] = '\0';
    return p;
}

static const Token* peek(Parser* p, int off) {
    int idx = p->pos + off;
    if (idx < 0) idx = 0;
    if (idx >= p->n) return &p->toks[p->n - 1];
    return &p->toks[idx];
}

static void fail(Parser* p, const char* msg) {
    if (!p->err) {
        p->err = 1;
        snprintf(p->emsg, p->ecap, "line %d: %s", peek(p, 0)->line, msg);
    }
}

static int accept(Parser* p, TokKind k) {
    if (peek(p, 0)->kind == k) { p->pos++; return 1; }
    return 0;
}

static int expect(Parser* p, TokKind k, const char* msg) {
    if (accept(p, k)) return 1;
    fail(p, msg);
    return 0;
}

static LAst* new_ast(AstKind k, int line) {
    LAst* a = (LAst*)calloc(1, sizeof(LAst));
    a->kind = k;
    a->line = line;
    return a;
}

static int is_block_end(TokKind k) {
    return k == TK_EOF || k == TK_END || k == TK_ELSE || k == TK_ELSEIF || k == TK_UNTIL;
}

static LAst* parse_expr(Parser* p);
static LAst* parse_block(Parser* p);

static void parse_explist(Parser* p, LAst*** out, int* nout) {
    LAst** arr = NULL; int cnt = 0, cap = 0;
    do {
        LAst* e = parse_expr(p);
        if (cnt + 1 >= cap) { cap = cap ? cap * 2 : 4; arr = (LAst**)realloc(arr, (size_t)cap * sizeof(LAst*)); }
        arr[cnt++] = e;
    } while (!p->err && accept(p, TK_COMMA));
    *out = arr; *nout = cnt;
}

static void parse_func_body(Parser* p, LAst* node, int implicit_self) {
    expect(p, TK_LPAREN, "expected '('");
    char** params = NULL; int np = 0, cap = 0;
    if (implicit_self) {
        cap = 4; params = (char**)malloc(4 * sizeof(char*));
        params[np++] = xstrdup("self", 4);
    }
    int vararg = 0;
    if (!p->err && peek(p, 0)->kind != TK_RPAREN) {
        for (;;) {
            if (accept(p, TK_DOTS)) { vararg = 1; break; }
            const Token* t = peek(p, 0);
            if (!expect(p, TK_NAME, "expected parameter name")) break;
            if (np + 1 >= cap) { cap = cap ? cap * 2 : 4; params = (char**)realloc(params, (size_t)cap * sizeof(char*)); }
            params[np++] = xstrdup(t->text, t->len);
            if (!accept(p, TK_COMMA)) break;
        }
    }
    expect(p, TK_RPAREN, "expected ')'");
    LAst* body = parse_block(p);
    expect(p, TK_END, "expected 'end'");
    node->u.func.params = params;
    node->u.func.nparams = np;
    node->u.func.has_vararg = vararg;
    node->u.func.body = body;
}

static LAst* parse_table(Parser* p) {
    int line = peek(p, 0)->line;
    expect(p, TK_LBRACE, "expected '{'");
    LAst* a = new_ast(AST_TABLE, line);
    AstField* f = NULL; int nf = 0, cap = 0;
    while (!p->err && peek(p, 0)->kind != TK_RBRACE) {
        AstField fld; memset(&fld, 0, sizeof(fld));
        if (accept(p, TK_LBRACK)) {
            fld.key = parse_expr(p);
            expect(p, TK_RBRACK, "expected ']'");
            expect(p, TK_ASSIGN, "expected '='");
            fld.val = parse_expr(p);
        } else if (peek(p, 0)->kind == TK_NAME && peek(p, 1)->kind == TK_ASSIGN) {
            const Token* nt = peek(p, 0);
            p->pos += 2;
            LAst* k = new_ast(AST_STR, nt->line);
            k->u.sval.s = xstrdup(nt->text, nt->len);
            k->u.sval.len = nt->len;
            fld.key = k;
            fld.val = parse_expr(p);
        } else {
            fld.key = NULL;
            fld.val = parse_expr(p);
        }
        if (nf + 1 >= cap) { cap = cap ? cap * 2 : 8; f = (AstField*)realloc(f, (size_t)cap * sizeof(AstField)); }
        f[nf++] = fld;
        if (!accept(p, TK_COMMA) && !accept(p, TK_SEMI)) break;
    }
    expect(p, TK_RBRACE, "expected '}'");
    a->u.tab.fields = f;
    a->u.tab.nfields = nf;
    return a;
}

static void parse_args(Parser* p, LAst*** args, int* nargs) {
    TokKind k = peek(p, 0)->kind;
    if (k == TK_LPAREN) {
        p->pos++;
        if (peek(p, 0)->kind != TK_RPAREN) parse_explist(p, args, nargs);
        else { *args = NULL; *nargs = 0; }
        expect(p, TK_RPAREN, "expected ')'");
    } else if (k == TK_STR) {
        const Token* t = peek(p, 0); p->pos++;
        LAst* s = new_ast(AST_STR, t->line);
        s->u.sval.s = xstrdup(t->text, t->len); s->u.sval.len = t->len;
        *args = (LAst**)malloc(sizeof(LAst*)); (*args)[0] = s; *nargs = 1;
    } else if (k == TK_LBRACE) {
        *args = (LAst**)malloc(sizeof(LAst*)); (*args)[0] = parse_table(p); *nargs = 1;
    } else {
        fail(p, "expected function arguments");
    }
}

static LAst* parse_suffixed(Parser* p) {
    const Token* t = peek(p, 0);
    LAst* e = NULL;
    if (accept(p, TK_LPAREN)) {
        e = parse_expr(p);
        expect(p, TK_RPAREN, "expected ')'");
    } else if (t->kind == TK_NAME) {
        p->pos++;
        e = new_ast(AST_VAR, t->line);
        e->u.var.name = xstrdup(t->text, t->len);
    } else {
        fail(p, "unexpected symbol");
        return new_ast(AST_NIL, t->line);
    }

    while (!p->err) {
        TokKind k = peek(p, 0)->kind;
        if (accept(p, TK_DOT)) {
            const Token* f = peek(p, 0);
            if (!expect(p, TK_NAME, "expected field name")) break;
            LAst* idx = new_ast(AST_INDEX, f->line);
            LAst* ks = new_ast(AST_STR, f->line);
            ks->u.sval.s = xstrdup(f->text, f->len); ks->u.sval.len = f->len;
            idx->u.idx.obj = e; idx->u.idx.key = ks;
            e = idx;
        } else if (accept(p, TK_LBRACK)) {
            LAst* idx = new_ast(AST_INDEX, t->line);
            idx->u.idx.obj = e; idx->u.idx.key = parse_expr(p);
            expect(p, TK_RBRACK, "expected ']'");
            e = idx;
        } else if (accept(p, TK_COLON)) {
            const Token* m = peek(p, 0);
            if (!expect(p, TK_NAME, "expected method name")) break;
            LAst* c = new_ast(AST_METHOD_CALL, m->line);
            c->u.call.fn = e;
            c->u.call.method = xstrdup(m->text, m->len);
            parse_args(p, &c->u.call.args, &c->u.call.nargs);
            e = c;
        } else if (k == TK_LPAREN || k == TK_STR || k == TK_LBRACE) {
            LAst* c = new_ast(AST_CALL, peek(p, 0)->line);
            c->u.call.fn = e;
            parse_args(p, &c->u.call.args, &c->u.call.nargs);
            e = c;
        } else break;
    }
    return e;
}

static LAst* parse_simple(Parser* p) {
    const Token* t = peek(p, 0);
    if (accept(p, TK_NIL)) return new_ast(AST_NIL, t->line);
    if (accept(p, TK_TRUE)) { LAst* a = new_ast(AST_BOOL, t->line); a->u.bval.b = 1; return a; }
    if (accept(p, TK_FALSE)) { LAst* a = new_ast(AST_BOOL, t->line); a->u.bval.b = 0; return a; }
    if (accept(p, TK_DOTS)) return new_ast(AST_VARARG, t->line);
    if (t->kind == TK_NUM) {
        p->pos++;
        LAst* a = new_ast(AST_NUM, t->line);
        a->u.nval.n = t->num; a->u.nval.i = t->inum; a->u.nval.is_int = t->is_int;
        return a;
    }
    if (t->kind == TK_STR) {
        p->pos++;
        LAst* a = new_ast(AST_STR, t->line);
        a->u.sval.s = xstrdup(t->text, t->len); a->u.sval.len = t->len;
        return a;
    }
    if (t->kind == TK_LBRACE) return parse_table(p);
    if (accept(p, TK_FUNCTION)) {
        LAst* a = new_ast(AST_FUNC_EXPR, t->line);
        snprintf(a->u.func.name, sizeof(a->u.func.name), "anon");
        parse_func_body(p, a, 0);
        return a;
    }
    return parse_suffixed(p);
}

static int binop_prec(TokKind k, int* rassoc) {
    *rassoc = 0;
    switch (k) {
    case TK_OR: return 1;
    case TK_AND: return 2;
    case TK_LT: case TK_GT: case TK_LE: case TK_GE: case TK_NE: case TK_EQ: return 3;
    case TK_CONCAT: *rassoc = 1; return 4;
    case TK_ADD: case TK_SUB: return 5;
    case TK_MUL: case TK_DIV: case TK_IDIV: case TK_MOD: return 6;
    case TK_POW: *rassoc = 1; return 8;
    default: return 0;
    }
}

static LAst* parse_subexpr(Parser* p, int min_prec) {
    const Token* t = peek(p, 0);
    LAst* lhs = NULL;
    if (t->kind == TK_SUB || t->kind == TK_NOT || t->kind == TK_LEN) {
        TokKind op = t->kind; p->pos++;
        lhs = new_ast(AST_UNOP, t->line);
        lhs->u.unop.op = op;
        lhs->u.unop.expr = parse_subexpr(p, 7);
    } else {
        lhs = parse_simple(p);
    }
    while (!p->err) {
        int rassoc = 0;
        TokKind op = peek(p, 0)->kind;
        int prec = binop_prec(op, &rassoc);
        if (prec < min_prec) break;
        int line = peek(p, 0)->line;
        p->pos++;
        LAst* rhs = parse_subexpr(p, rassoc ? prec : prec + 1);
        LAst* b = new_ast(AST_BINOP, line);
        b->u.binop.op = op; b->u.binop.left = lhs; b->u.binop.right = rhs;
        lhs = b;
    }
    return lhs;
}

static LAst* parse_expr(Parser* p) { return parse_subexpr(p, 1); }

static LAst* parse_stmt(Parser* p) {
    const Token* t = peek(p, 0);
    if (accept(p, TK_SEMI)) return NULL;
    if (accept(p, TK_BREAK)) return new_ast(AST_BREAK, t->line);
    if (accept(p, TK_RETURN)) {
        LAst* r = new_ast(AST_RETURN, t->line);
        TokKind nk = peek(p, 0)->kind;
        if (!is_block_end(nk) && nk != TK_SEMI) parse_explist(p, &r->u.ret.vals, &r->u.ret.nvals);
        accept(p, TK_SEMI);
        return r;
    }
    if (accept(p, TK_DO)) {
        LAst* b = new_ast(AST_DO, t->line);
        b->u.loop.body = parse_block(p);
        expect(p, TK_END, "expected 'end'");
        return b;
    }
    if (accept(p, TK_WHILE)) {
        LAst* w = new_ast(AST_WHILE, t->line);
        w->u.loop.cond = parse_expr(p);
        expect(p, TK_DO, "expected 'do'");
        w->u.loop.body = parse_block(p);
        expect(p, TK_END, "expected 'end'");
        return w;
    }
    if (accept(p, TK_REPEAT)) {
        LAst* r = new_ast(AST_REPEAT, t->line);
        r->u.loop.body = parse_block(p);
        expect(p, TK_UNTIL, "expected 'until'");
        r->u.loop.cond = parse_expr(p);
        return r;
    }
    if (accept(p, TK_IF)) {
        LAst* a = new_ast(AST_IF, t->line);
        AstBranch* br = NULL; int nb = 0, cap = 0;
        do {
            LAst* cond = parse_expr(p);
            expect(p, TK_THEN, "expected 'then'");
            LAst* body = parse_block(p);
            if (nb + 1 >= cap) { cap = cap ? cap * 2 : 4; br = (AstBranch*)realloc(br, (size_t)cap * sizeof(AstBranch)); }
            br[nb].cond = cond; br[nb].body = body; nb++;
        } while (!p->err && accept(p, TK_ELSEIF));
        a->u.ifs.branches = br; a->u.ifs.nbranches = nb;
        if (accept(p, TK_ELSE)) a->u.ifs.else_body = parse_block(p);
        expect(p, TK_END, "expected 'end'");
        return a;
    }
    if (accept(p, TK_FOR)) {
        const Token* v = peek(p, 0);
        expect(p, TK_NAME, "expected variable name");
        if (accept(p, TK_ASSIGN)) {
            LAst* f = new_ast(AST_FOR_NUM, t->line);
            f->u.fornum.var = xstrdup(v->text, v->len);
            f->u.fornum.start = parse_expr(p);
            expect(p, TK_COMMA, "expected ','");
            f->u.fornum.stop = parse_expr(p);
            if (accept(p, TK_COMMA)) f->u.fornum.step = parse_expr(p);
            expect(p, TK_DO, "expected 'do'");
            f->u.fornum.body = parse_block(p);
            expect(p, TK_END, "expected 'end'");
            return f;
        }
        LAst* f = new_ast(AST_FOR_IN, t->line);
        char** vars = (char**)malloc(4 * sizeof(char*)); int nv = 0, cap = 4;
        vars[nv++] = xstrdup(v->text, v->len);
        while (!p->err && accept(p, TK_COMMA)) {
            const Token* nx = peek(p, 0);
            if (!expect(p, TK_NAME, "expected variable name")) break;
            if (nv + 1 >= cap) { cap *= 2; vars = (char**)realloc(vars, (size_t)cap * sizeof(char*)); }
            vars[nv++] = xstrdup(nx->text, nx->len);
        }
        expect(p, TK_IN, "expected 'in'");
        parse_explist(p, &f->u.forin.iters, &f->u.forin.niters);
        expect(p, TK_DO, "expected 'do'");
        f->u.forin.vars = vars; f->u.forin.nvars = nv;
        f->u.forin.body = parse_block(p);
        expect(p, TK_END, "expected 'end'");
        return f;
    }
    if (accept(p, TK_LOCAL)) {
        if (accept(p, TK_FUNCTION)) {
            const Token* nt = peek(p, 0);
            expect(p, TK_NAME, "expected function name");
            LAst* fn = new_ast(AST_FUNC_EXPR, t->line);
            snprintf(fn->u.func.name, sizeof(fn->u.func.name), "%s", nt->text ? nt->text : "fn");
            parse_func_body(p, fn, 0);
            LAst* lc = new_ast(AST_LOCAL, t->line);
            lc->u.local.names = (char**)malloc(sizeof(char*));
            lc->u.local.names[0] = xstrdup(nt->text ? nt->text : "", nt->len);
            lc->u.local.nnames = 1;
            lc->u.local.rhs = (LAst**)malloc(sizeof(LAst*));
            lc->u.local.rhs[0] = fn; lc->u.local.nrhs = 1;
            return lc;
        }
        LAst* lc = new_ast(AST_LOCAL, t->line);
        char** names = NULL; int nn = 0, cap = 0;
        do {
            const Token* nt = peek(p, 0);
            if (!expect(p, TK_NAME, "expected local name")) break;
            if (nn + 1 >= cap) { cap = cap ? cap * 2 : 4; names = (char**)realloc(names, (size_t)cap * sizeof(char*)); }
            names[nn++] = xstrdup(nt->text, nt->len);
        } while (!p->err && accept(p, TK_COMMA));
        lc->u.local.names = names; lc->u.local.nnames = nn;
        if (accept(p, TK_ASSIGN)) parse_explist(p, &lc->u.local.rhs, &lc->u.local.nrhs);
        return lc;
    }
    if (accept(p, TK_FUNCTION)) {
        const Token* base = peek(p, 0);
        expect(p, TK_NAME, "expected function name");
        LAst* target = new_ast(AST_VAR, base->line);
        target->u.var.name = xstrdup(base->text ? base->text : "", base->len);
        int is_method = 0;
        while (!p->err && accept(p, TK_DOT)) {
            const Token* f = peek(p, 0);
            if (!expect(p, TK_NAME, "expected field name")) break;
            LAst* idx = new_ast(AST_INDEX, f->line);
            LAst* ks = new_ast(AST_STR, f->line);
            ks->u.sval.s = xstrdup(f->text, f->len); ks->u.sval.len = f->len;
            idx->u.idx.obj = target; idx->u.idx.key = ks;
            target = idx;
        }
        if (!p->err && accept(p, TK_COLON)) {
            const Token* m = peek(p, 0);
            expect(p, TK_NAME, "expected method name");
            LAst* idx = new_ast(AST_INDEX, m->line);
            LAst* ks = new_ast(AST_STR, m->line);
            ks->u.sval.s = xstrdup(m->text ? m->text : "", m->len); ks->u.sval.len = m->len;
            idx->u.idx.obj = target; idx->u.idx.key = ks;
            target = idx;
            is_method = 1;
        }
        LAst* fn = new_ast(AST_FUNC_EXPR, t->line);
        snprintf(fn->u.func.name, sizeof(fn->u.func.name), "%s", base->text ? base->text : "fn");
        parse_func_body(p, fn, is_method);
        LAst* as = new_ast(AST_ASSIGN, t->line);
        as->u.assign.lhs = (LAst**)malloc(sizeof(LAst*)); as->u.assign.lhs[0] = target; as->u.assign.nlhs = 1;
        as->u.assign.rhs = (LAst**)malloc(sizeof(LAst*)); as->u.assign.rhs[0] = fn; as->u.assign.nrhs = 1;
        return as;
    }

    LAst* first = parse_suffixed(p);
    if (first->kind == AST_CALL || first->kind == AST_METHOD_CALL) return first;
    LAst** lhs = (LAst**)malloc(4 * sizeof(LAst*)); int nlhs = 0, cap = 4;
    lhs[nlhs++] = first;
    while (!p->err && accept(p, TK_COMMA)) {
        if (nlhs + 1 >= cap) { cap *= 2; lhs = (LAst**)realloc(lhs, (size_t)cap * sizeof(LAst*)); }
        lhs[nlhs++] = parse_suffixed(p);
    }
    expect(p, TK_ASSIGN, "expected '=' or function call");
    LAst* as = new_ast(AST_ASSIGN, t->line);
    as->u.assign.lhs = lhs; as->u.assign.nlhs = nlhs;
    parse_explist(p, &as->u.assign.rhs, &as->u.assign.nrhs);
    return as;
}

static LAst* parse_block(Parser* p) {
    LAst* blk = new_ast(AST_BLOCK, peek(p, 0)->line);
    LAst** arr = NULL; int ns = 0, cap = 0;
    while (!p->err && !is_block_end(peek(p, 0)->kind)) {
        LAst* s = parse_stmt(p);
        if (s) {
            if (ns + 1 >= cap) { cap = cap ? cap * 2 : 8; arr = (LAst**)realloc(arr, (size_t)cap * sizeof(LAst*)); }
            arr[ns++] = s;
        }
    }
    blk->u.block.stmts = arr;
    blk->u.block.nstmts = ns;
    return blk;
}

LAst* lua_parse(const Token* toks, int ntoks, char* err, size_t err_cap) {
    Parser p; memset(&p, 0, sizeof(p));
    p.toks = toks; p.n = ntoks; p.emsg = err; p.ecap = err_cap;
    LAst* root = parse_block(&p);
    if (!p.err && peek(&p, 0)->kind != TK_EOF) fail(&p, "unexpected trailing token");
    if (p.err) { lua_free_ast(root); return NULL; }
    return root;
}

void lua_free_ast(LAst* n) {
    if (!n) return;
    switch (n->kind) {
    case AST_BLOCK:
        for (int i = 0; i < n->u.block.nstmts; i++) lua_free_ast(n->u.block.stmts[i]);
        free(n->u.block.stmts); break;
    case AST_STR: free(n->u.sval.s); break;
    case AST_VAR: free(n->u.var.name); break;
    case AST_TABLE:
        for (int i = 0; i < n->u.tab.nfields; i++) { lua_free_ast(n->u.tab.fields[i].key); lua_free_ast(n->u.tab.fields[i].val); }
        free(n->u.tab.fields); break;
    case AST_INDEX: lua_free_ast(n->u.idx.obj); lua_free_ast(n->u.idx.key); break;
    case AST_UNOP: lua_free_ast(n->u.unop.expr); break;
    case AST_BINOP: lua_free_ast(n->u.binop.left); lua_free_ast(n->u.binop.right); break;
    case AST_CALL: case AST_METHOD_CALL:
        lua_free_ast(n->u.call.fn); free(n->u.call.method);
        for (int i = 0; i < n->u.call.nargs; i++) lua_free_ast(n->u.call.args[i]);
        free(n->u.call.args); break;
    case AST_FUNC_EXPR:
        for (int i = 0; i < n->u.func.nparams; i++) free(n->u.func.params[i]);
        free(n->u.func.params); lua_free_ast(n->u.func.body); break;
    case AST_ASSIGN:
        for (int i = 0; i < n->u.assign.nlhs; i++) lua_free_ast(n->u.assign.lhs[i]);
        for (int i = 0; i < n->u.assign.nrhs; i++) lua_free_ast(n->u.assign.rhs[i]);
        free(n->u.assign.lhs); free(n->u.assign.rhs); break;
    case AST_LOCAL:
        for (int i = 0; i < n->u.local.nnames; i++) free(n->u.local.names[i]);
        for (int i = 0; i < n->u.local.nrhs; i++) lua_free_ast(n->u.local.rhs[i]);
        free(n->u.local.names); free(n->u.local.rhs); break;
    case AST_IF:
        for (int i = 0; i < n->u.ifs.nbranches; i++) { lua_free_ast(n->u.ifs.branches[i].cond); lua_free_ast(n->u.ifs.branches[i].body); }
        free(n->u.ifs.branches); lua_free_ast(n->u.ifs.else_body); break;
    case AST_WHILE: case AST_REPEAT: case AST_DO:
        lua_free_ast(n->u.loop.cond); lua_free_ast(n->u.loop.body); break;
    case AST_FOR_NUM:
        free(n->u.fornum.var); lua_free_ast(n->u.fornum.start); lua_free_ast(n->u.fornum.stop);
        lua_free_ast(n->u.fornum.step); lua_free_ast(n->u.fornum.body); break;
    case AST_FOR_IN:
        for (int i = 0; i < n->u.forin.nvars; i++) free(n->u.forin.vars[i]);
        for (int i = 0; i < n->u.forin.niters; i++) lua_free_ast(n->u.forin.iters[i]);
        free(n->u.forin.vars); free(n->u.forin.iters); lua_free_ast(n->u.forin.body); break;
    case AST_RETURN:
        for (int i = 0; i < n->u.ret.nvals; i++) lua_free_ast(n->u.ret.vals[i]);
        free(n->u.ret.vals); break;
    default: break;
    }
    free(n);
}
