#include "lua.h"
#include <math.h>
#include <stdlib.h>
#include <string.h>

static LVal eval_expr(LuaVM* vm, LAst* e);
static LVal eval_index_get(LuaVM* vm, LVal obj, LVal key);

static int eval_multi(LuaVM* vm, LAst** exprs, int nexprs, LVal* out, int want) {
    int pos = 0;
    for (int i = 0; i < nexprs && pos < want; i++) {
        LAst* ex = exprs[i];
        if (i == nexprs - 1 && (ex->kind == AST_CALL || ex->kind == AST_METHOD_CALL || ex->kind == AST_VARARG)) {
            if (ex->kind == AST_VARARG) {
                for (int k = 0; k < vm->nvarargs && pos < want; k++) out[pos++] = vm->varargs[k];
            } else {
                LVal tmp[16];
                LVal fn = eval_expr(vm, ex->u.call.fn);
                if (vm->err_flag) return 0;
                LVal args[32]; int na = 0;
                if (ex->kind == AST_METHOD_CALL) {
                    args[na++] = fn;
                    LStr* ms = lstr_new(vm, ex->u.call.method, strlen(ex->u.call.method));
                    LVal mkey; mkey.type = LVAL_STR; mkey.is_int = 0; mkey.u.s = ms;
                    fn = eval_index_get(vm, fn, mkey);
                    if (vm->err_flag) return 0;
                }
                for (int a = 0; a < ex->u.call.nargs && na < 31; a++) {
                    int base = vm->nroots;
                    lua_gc_push_root(vm, fn);
                    for (int k = 0; k < na; k++) lua_gc_push_root(vm, args[k]);
                    args[na++] = eval_expr(vm, ex->u.call.args[a]);
                    vm->nroots = base;
                    if (vm->err_flag) return 0;
                }
                int got = lua_call_val(vm, fn, na, args, tmp, 16);
                if (vm->err_flag) return 0;
                for (int k = 0; k < got && pos < want; k++) out[pos++] = tmp[k];
            }
        } else {
            out[pos++] = eval_expr(vm, ex);
            if (vm->err_flag) return 0;
        }
    }
    int actual = pos;
    while (pos < want) out[pos++] = lval_nil();
    return actual;
}

static LVal eval_binop(LuaVM* vm, TokKind op, LVal l, LVal r) {
    if (op == TK_EQ) return lval_bool(lval_eq(l, r));
    if (op == TK_NE) return lval_bool(!lval_eq(l, r));
    if (op == TK_CONCAT) {
        char bl[256], br[256];
        lua_fmt_val(l, bl, sizeof(bl));
        lua_fmt_val(r, br, sizeof(br));
        size_t ll = (l.type == LVAL_STR) ? l.u.s->len : strlen(bl);
        size_t lr = (r.type == LVAL_STR) ? r.u.s->len : strlen(br);
        char* tmp = (char*)malloc(ll + lr + 1);
        memcpy(tmp, (l.type == LVAL_STR) ? l.u.s->data : bl, ll);
        memcpy(tmp + ll, (r.type == LVAL_STR) ? r.u.s->data : br, lr);
        LStr* s = lstr_new(vm, tmp, ll + lr);
        free(tmp);
        LVal v; v.type = LVAL_STR; v.is_int = 0; v.u.s = s; return v;
    }
    if (op == TK_LT || op == TK_LE || op == TK_GT || op == TK_GE) {
        if (l.type == LVAL_STR && r.type == LVAL_STR) {
            int c = strcmp(l.u.s->data, r.u.s->data);
            return lval_bool(op == TK_LT ? c < 0 : op == TK_LE ? c <= 0 : op == TK_GT ? c > 0 : c >= 0);
        }
        int ok1 = 0, ok2 = 0;
        double a = lval_as_num(l, &ok1), b = lval_as_num(r, &ok2);
        if (!ok1 || !ok2) { lua_set_error(vm, "attempt to compare non-numbers"); return lval_nil(); }
        return lval_bool(op == TK_LT ? a < b : op == TK_LE ? a <= b : op == TK_GT ? a > b : a >= b);
    }

    int ok1 = 0, ok2 = 0;
    double a = lval_as_num(l, &ok1), b = lval_as_num(r, &ok2);
    if (!ok1 || !ok2) { lua_set_error(vm, "attempt to perform arithmetic on non-number"); return lval_nil(); }
    if (l.type == LVAL_NUM && r.type == LVAL_NUM && l.is_int && r.is_int && op != TK_DIV && op != TK_POW) {
        int64_t ia = l.u.i, ib = r.u.i;
        if ((op == TK_IDIV || op == TK_MOD) && ib == 0) { lua_set_error(vm, "attempt to divide by zero"); return lval_nil(); }
        switch (op) {
        case TK_ADD: return lval_int(ia + ib);
        case TK_SUB: return lval_int(ia - ib);
        case TK_MUL: return lval_int(ia * ib);
        case TK_IDIV: {
            if (ia == INT64_MIN && ib == -1) return lval_int(INT64_MIN);
            int64_t q = ia / ib;
            if ((ia % ib) != 0 && ((ia ^ ib) < 0)) q--;
            return lval_int(q);
        }
        case TK_MOD: { int64_t m = ia % ib; if (m && ((m < 0) != (ib < 0))) m += ib; return lval_int(m); }
        default: break;
        }
    }
    switch (op) {
    case TK_ADD: return lval_num(a + b);
    case TK_SUB: return lval_num(a - b);
    case TK_MUL: return lval_num(a * b);
    case TK_DIV: return lval_num(a / b);
    case TK_IDIV: return lval_num(floor(a / b));
    case TK_MOD: return lval_num(a - floor(a / b) * b);
    case TK_POW: return lval_num(pow(a, b));
    default: return lval_nil();
    }
}

static LVal eval_index_get(LuaVM* vm, LVal obj, LVal key) {
    if (obj.type == LVAL_TABLE) {
        LVal v = ltab_get(obj.u.t, key);
        if (v.type == LVAL_NIL && obj.u.t->metatable) {
            LStr* is = lstr_new(vm, "__index", 7);
            LVal ik; ik.type = LVAL_STR; ik.is_int = 0; ik.u.s = is;
            LVal idx = ltab_get(obj.u.t->metatable, ik);
            if (idx.type == LVAL_TABLE) return ltab_get(idx.u.t, key);
        }
        return v;
    }
    if (obj.type == LVAL_STR && vm->string_mt) return ltab_get(vm->string_mt, key);
    if (obj.type == LVAL_FILE) {
        LStr* fs = lstr_new(vm, "__file_mt", 9);
        LVal fmt = lenv_get(vm->globals, fs);
        if (fmt.type == LVAL_TABLE) return ltab_get(fmt.u.t, key);
    }
    lua_set_error(vm, "attempt to index a non-table value");
    return lval_nil();
}

static LVal eval_expr(LuaVM* vm, LAst* e) {
    if (!e || vm->err_flag) return lval_nil();
    switch (e->kind) {
    case AST_NIL: return lval_nil();
    case AST_BOOL: return lval_bool(e->u.bval.b);
    case AST_NUM: return e->u.nval.is_int ? lval_int(e->u.nval.i) : lval_num(e->u.nval.n);
    case AST_STR: { LVal v; v.type = LVAL_STR; v.is_int = 0; v.u.s = lstr_new(vm, e->u.sval.s, e->u.sval.len); return v; }
    case AST_VARARG: return vm->nvarargs > 0 ? vm->varargs[0] : lval_nil();
    case AST_VAR: {
        LStr* nm = lstr_new(vm, e->u.var.name, strlen(e->u.var.name));
        return lenv_get(vm->cur_env, nm);
    }
    case AST_TABLE: {
        LTable* t = ltab_new(vm);
        LVal tv; tv.type = LVAL_TABLE; tv.is_int = 0; tv.u.t = t;
        lua_gc_push_root(vm, tv);
        int arr_idx = 1;
        for (int i = 0; i < e->u.tab.nfields; i++) {
            if (!e->u.tab.fields[i].key) {
                LVal v = eval_expr(vm, e->u.tab.fields[i].val);
                ltab_set(vm, t, lval_int(arr_idx++), v);
            } else {
                LVal k = eval_expr(vm, e->u.tab.fields[i].key);
                LVal v = eval_expr(vm, e->u.tab.fields[i].val);
                ltab_set(vm, t, k, v);
            }
        }
        lua_gc_pop_roots(vm, 1);
        return tv;
    }
    case AST_INDEX: {
        LVal obj = eval_expr(vm, e->u.idx.obj);
        LVal key = eval_expr(vm, e->u.idx.key);
        return eval_index_get(vm, obj, key);
    }
    case AST_UNOP: {
        LVal v = eval_expr(vm, e->u.unop.expr);
        if (e->u.unop.op == TK_NOT) return lval_bool(!lval_truthy(v));
        if (e->u.unop.op == TK_LEN) {
            if (v.type == LVAL_STR) return lval_int((int64_t)v.u.s->len);
            if (v.type == LVAL_TABLE) return lval_int(ltab_len(v.u.t));
            lua_set_error(vm, "attempt to get length of invalid value");
            return lval_nil();
        }
        int ok = 0; double d = lval_as_num(v, &ok);
        if (!ok) { lua_set_error(vm, "attempt to negate non-number"); return lval_nil(); }
        return v.is_int ? lval_int(-v.u.i) : lval_num(-d);
    }
    case AST_BINOP: {
        if (e->u.binop.op == TK_AND) {
            LVal l = eval_expr(vm, e->u.binop.left);
            return lval_truthy(l) ? eval_expr(vm, e->u.binop.right) : l;
        }
        if (e->u.binop.op == TK_OR) {
            LVal l = eval_expr(vm, e->u.binop.left);
            return lval_truthy(l) ? l : eval_expr(vm, e->u.binop.right);
        }
        LVal l = eval_expr(vm, e->u.binop.left);
        lua_gc_push_root(vm, l);
        LVal r = eval_expr(vm, e->u.binop.right);
        lua_gc_pop_roots(vm, 1);
        return vm->err_flag ? lval_nil() : eval_binop(vm, e->u.binop.op, l, r);
    }
    case AST_FUNC_EXPR: {
        LFunc* fn = lfunc_new(vm);
        fn->params = e->u.func.params; fn->nparams = e->u.func.nparams;
        fn->has_vararg = e->u.func.has_vararg; fn->body = e->u.func.body;
        fn->closure = vm->cur_env;
        snprintf(fn->name, sizeof(fn->name), "%s", e->u.func.name);
        LVal v; v.type = LVAL_FUNC; v.is_int = 0; v.u.f = fn; return v;
    }
    case AST_CALL: case AST_METHOD_CALL: {
        LVal out = lval_nil();
        LVal fn = eval_expr(vm, e->u.call.fn);
        if (vm->err_flag) return lval_nil();
        LVal args[32]; int na = 0;
        if (e->kind == AST_METHOD_CALL) {
            args[na++] = fn;
            LStr* ms = lstr_new(vm, e->u.call.method, strlen(e->u.call.method));
            LVal mk; mk.type = LVAL_STR; mk.is_int = 0; mk.u.s = ms;
            fn = eval_index_get(vm, fn, mk);
        }
        for (int i = 0; i < e->u.call.nargs && na < 31; i++) {
            lua_gc_push_root(vm, fn);
            for (int k = 0; k < na; k++) lua_gc_push_root(vm, args[k]);
            args[na++] = eval_expr(vm, e->u.call.args[i]);
            lua_gc_pop_roots(vm, na);
            if (vm->err_flag) return lval_nil();
        }
        lua_call_val(vm, fn, na, args, &out, 1);
        return out;
    }
    default: return lval_nil();
    }
}

int lua_call_val(LuaVM* vm, LVal fn, int nargs, LVal* args, LVal* out, int max_out) {
    if (vm->err_flag) return 0;
    int base_roots = vm->nroots;
    lua_gc_push_root(vm, fn);
    for (int i = 0; i < nargs; i++) lua_gc_push_root(vm, args[i]);
    if (fn.type == LVAL_CFUNC) {
        int r = fn.u.cf(vm, nargs, args, out, max_out);
        vm->nroots = base_roots;
        return r < max_out ? r : max_out;
    }
    if (fn.type != LVAL_FUNC) {
        vm->nroots = base_roots;
        lua_set_error(vm, "attempt to call a non-function value");
        return 0;
    }
    LFunc* f = fn.u.f;
    LEnv* saved_env = vm->cur_env;
    LVal* saved_va = vm->varargs; int saved_nva = vm->nvarargs;
    int saved_ret = vm->returning, saved_brk = vm->breaking;

    LEnv* fenv = lenv_new(vm, f->closure ? f->closure : vm->globals);
    for (int i = 0; i < f->nparams; i++) {
        LStr* ps = lstr_new(vm, f->params[i], strlen(f->params[i]));
        lenv_set_local(vm, fenv, ps, i < nargs ? args[i] : lval_nil());
    }
    vm->varargs = (nargs > f->nparams) ? &args[f->nparams] : NULL;
    vm->nvarargs = (nargs > f->nparams) ? (nargs - f->nparams) : 0;
    vm->cur_env = fenv;
    vm->returning = 0; vm->breaking = 0; vm->nret = 0;

    lua_exec_ast(vm, f->body);

    int got = 0;
    if (vm->returning) {
        got = vm->nret < max_out ? vm->nret : max_out;
        for (int i = 0; i < got; i++) out[i] = vm->ret_vals[i];
    }
    vm->cur_env = saved_env;
    vm->varargs = saved_va; vm->nvarargs = saved_nva;
    vm->returning = saved_ret; vm->breaking = saved_brk;
    vm->nroots = base_roots;
    return got;
}

static void assign_lhs(LuaVM* vm, LAst* l, LVal v) {
    if (l->kind == AST_VAR) {
        LStr* nm = lstr_new(vm, l->u.var.name, strlen(l->u.var.name));
        lenv_assign(vm, vm->cur_env, nm, v);
    } else if (l->kind == AST_INDEX) {
        LVal obj = eval_expr(vm, l->u.idx.obj);
        LVal key = eval_expr(vm, l->u.idx.key);
        if (obj.type == LVAL_TABLE) ltab_set(vm, obj.u.t, key, v);
        else lua_set_error(vm, "attempt to index non-table for assignment");
    }
}

int lua_exec_ast(LuaVM* vm, LAst* node) {
    if (!node || vm->err_flag || vm->returning || vm->breaking) return 0;
    switch (node->kind) {
    case AST_BLOCK:
        for (int i = 0; i < node->u.block.nstmts && !vm->err_flag && !vm->returning && !vm->breaking; i++)
            lua_exec_ast(vm, node->u.block.stmts[i]);
        break;
    case AST_DO: {
        LEnv* saved = vm->cur_env;
        vm->cur_env = lenv_new(vm, saved);
        lua_exec_ast(vm, node->u.loop.body);
        vm->cur_env = saved;
        break;
    }
    case AST_LOCAL: {
        LVal vals[32];
        int nn = node->u.local.nnames;
        if (nn > 32) nn = 32;
        if (node->u.local.nrhs > 0) eval_multi(vm, node->u.local.rhs, node->u.local.nrhs, vals, nn);
        else for (int i = 0; i < nn; i++) vals[i] = lval_nil();
        for (int i = 0; i < nn; i++) {
            LStr* nm = lstr_new(vm, node->u.local.names[i], strlen(node->u.local.names[i]));
            lenv_set_local(vm, vm->cur_env, nm, vals[i]);
        }
        break;
    }
    case AST_ASSIGN: {
        LVal vals[32];
        int nl = node->u.assign.nlhs;
        if (nl > 32) nl = 32;
        eval_multi(vm, node->u.assign.rhs, node->u.assign.nrhs, vals, nl);
        for (int i = 0; i < nl; i++) assign_lhs(vm, node->u.assign.lhs[i], vals[i]);
        break;
    }
    case AST_CALL: case AST_METHOD_CALL:
        eval_expr(vm, node);
        break;
    case AST_RETURN:
        vm->nret = 0;
        if (node->u.ret.nvals > 0) {
            vm->nret = eval_multi(vm, node->u.ret.vals, node->u.ret.nvals, vm->ret_vals, 16);
        }
        vm->returning = 1;
        break;
    case AST_BREAK: vm->breaking = 1; break;
    case AST_IF:
        for (int i = 0; i < node->u.ifs.nbranches; i++) {
            if (lval_truthy(eval_expr(vm, node->u.ifs.branches[i].cond))) {
                LEnv* saved = vm->cur_env;
                vm->cur_env = lenv_new(vm, saved);
                lua_exec_ast(vm, node->u.ifs.branches[i].body);
                vm->cur_env = saved;
                return 0;
            }
        }
        if (node->u.ifs.else_body) {
            LEnv* saved = vm->cur_env;
            vm->cur_env = lenv_new(vm, saved);
            lua_exec_ast(vm, node->u.ifs.else_body);
            vm->cur_env = saved;
        }
        break;
    case AST_WHILE:
        while (!vm->err_flag && !vm->returning && lval_truthy(eval_expr(vm, node->u.loop.cond))) {
            LEnv* saved = vm->cur_env;
            vm->cur_env = lenv_new(vm, saved);
            lua_exec_ast(vm, node->u.loop.body);
            vm->cur_env = saved;
            if (vm->breaking) { vm->breaking = 0; break; }
        }
        break;
    case AST_REPEAT: {
        for (;;) {
            LEnv* saved = vm->cur_env;
            vm->cur_env = lenv_new(vm, saved);
            lua_exec_ast(vm, node->u.loop.body);
            int done = vm->err_flag || vm->returning || vm->breaking || lval_truthy(eval_expr(vm, node->u.loop.cond));
            vm->cur_env = saved;
            if (vm->breaking) { vm->breaking = 0; break; }
            if (done) break;
        }
        break;
    }
    case AST_FOR_NUM: {
        int ok1 = 0, ok2 = 0, ok3 = 1;
        double a = lval_as_num(eval_expr(vm, node->u.fornum.start), &ok1);
        double b = lval_as_num(eval_expr(vm, node->u.fornum.stop), &ok2);
        double st = node->u.fornum.step ? lval_as_num(eval_expr(vm, node->u.fornum.step), &ok3) : 1.0;
        if (!ok1 || !ok2 || !ok3 || st == 0.0) { lua_set_error(vm, "invalid numeric 'for' limits"); break; }
        for (double cur = a; (st > 0) ? (cur <= b) : (cur >= b); cur += st) {
            if (vm->err_flag || vm->returning) break;
            LEnv* saved = vm->cur_env;
            vm->cur_env = lenv_new(vm, saved);
            LStr* vn = lstr_new(vm, node->u.fornum.var, strlen(node->u.fornum.var));
            LVal iv = (floor(cur) == cur && floor(st) == st && fabs(cur) < 1e15) ? lval_int((int64_t)cur) : lval_num(cur);
            lenv_set_local(vm, vm->cur_env, vn, iv);
            lua_exec_ast(vm, node->u.fornum.body);
            vm->cur_env = saved;
            if (vm->breaking) { vm->breaking = 0; break; }
        }
        break;
    }
    case AST_FOR_IN: {
        LVal ivals[4];
        eval_multi(vm, node->u.forin.iters, node->u.forin.niters, ivals, 4);
        LVal f = ivals[0], s = ivals[1], var = ivals[2];
        while (!vm->err_flag && !vm->returning) {
            LVal res[16]; LVal cargs[2] = {s, var};
            for (int k = 0; k < 16; k++) res[k] = lval_nil();
            lua_gc_push_root(vm, f); lua_gc_push_root(vm, s); lua_gc_push_root(vm, var);
            lua_call_val(vm, f, 2, cargs, res, 16);
            lua_gc_pop_roots(vm, 3);
            if (vm->err_flag || res[0].type == LVAL_NIL) break;
            var = res[0];
            LEnv* saved = vm->cur_env;
            vm->cur_env = lenv_new(vm, saved);
            for (int i = 0; i < node->u.forin.nvars; i++) {
                LStr* vn = lstr_new(vm, node->u.forin.vars[i], strlen(node->u.forin.vars[i]));
                lenv_set_local(vm, vm->cur_env, vn, i < 16 ? res[i] : lval_nil());
            }
            lua_exec_ast(vm, node->u.forin.body);
            vm->cur_env = saved;
            if (vm->breaking) { vm->breaking = 0; break; }
        }
        break;
    }
    default: break;
    }
    return 0;
}
