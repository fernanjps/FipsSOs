#include "lua.h"
#include <ctype.h>
#include <stdlib.h>
#include <string.h>

typedef struct { const char* name; TokKind kind; } Kw;
static const Kw KEYWORDS[] = {
    {"and", TK_AND}, {"break", TK_BREAK}, {"do", TK_DO}, {"else", TK_ELSE},
    {"elseif", TK_ELSEIF}, {"end", TK_END}, {"false", TK_FALSE}, {"for", TK_FOR},
    {"function", TK_FUNCTION}, {"if", TK_IF}, {"in", TK_IN}, {"local", TK_LOCAL},
    {"nil", TK_NIL}, {"not", TK_NOT}, {"or", TK_OR}, {"repeat", TK_REPEAT},
    {"return", TK_RETURN}, {"then", TK_THEN}, {"true", TK_TRUE}, {"until", TK_UNTIL},
    {"while", TK_WHILE}, {NULL, TK_EOF}
};

static int match_long_eq(const char* p, int* out_eq) {
    if (*p != '[') return 0;
    int eq = 0;
    const char* q = p + 1;
    while (*q == '=') { eq++; q++; }
    if (*q == '[') { *out_eq = eq; return (int)(q - p + 1); }
    return 0;
}

static const char* read_long_bracket(const char* p, int eq, int* line, char** out, size_t* olen) {
    if (*p == '\n') { (*line)++; p++; }
    size_t cap = 64, len = 0;
    char* buf = (char*)malloc(cap);
    while (*p) {
        if (*p == ']') {
            int k = 0;
            const char* q = p + 1;
            while (k < eq && *q == '=') { k++; q++; }
            if (k == eq && *q == ']') {
                buf[len] = '\0';
                if (out) { *out = buf; *olen = len; } else free(buf);
                return q + 1;
            }
        }
        if (*p == '\n') (*line)++;
        if (len + 2 >= cap) { cap *= 2; buf = (char*)realloc(buf, cap); }
        buf[len++] = *p++;
    }
    free(buf);
    return NULL;
}

static const char* read_string(const char* p, char q, int* line, char** out, size_t* olen, char* err, size_t ecap) {
    p++;
    size_t cap = 64, len = 0;
    char* buf = (char*)malloc(cap);
    while (*p && *p != q) {
        if (*p == '\n' || *p == '\r') {
            free(buf);
            snprintf(err, ecap, "line %d: unfinished string", *line);
            return NULL;
        }
        char ch = *p++;
        if (ch == '\\') {
            if (!*p) break;
            char esc = *p++;
            switch (esc) {
            case 'n': ch = '\n'; break;
            case 'r': ch = '\r'; break;
            case 't': ch = '\t'; break;
            case 'a': ch = '\a'; break;
            case 'b': ch = '\b'; break;
            case '\\': ch = '\\'; break;
            case '\'': ch = '\''; break;
            case '"': ch = '"'; break;
            case '0': ch = '\0'; break;
            case '\n': (*line)++; ch = '\n'; break;
            case 'x': {
                int v = 0;
                for (int k = 0; k < 2 && isxdigit((unsigned char)*p); k++, p++) {
                    v = (v << 4) | (isdigit((unsigned char)*p) ? (*p - '0') : (tolower((unsigned char)*p) - 'a' + 10));
                }
                ch = (char)v;
                break;
            }
            default:
                if (isdigit((unsigned char)esc)) {
                    int v = esc - '0';
                    for (int k = 0; k < 2 && isdigit((unsigned char)*p); k++, p++) v = v * 10 + (*p - '0');
                    ch = (char)v;
                } else {
                    ch = esc;
                }
                break;
            }
        }
        if (len + 2 >= cap) { cap *= 2; buf = (char*)realloc(buf, cap); }
        buf[len++] = ch;
    }
    if (*p != q) {
        free(buf);
        snprintf(err, ecap, "line %d: unfinished string", *line);
        return NULL;
    }
    buf[len] = '\0';
    *out = buf;
    *olen = len;
    return p + 1;
}

static void push_tok(Token** arr, int* cnt, int* cap, Token tk) {
    if (*cnt + 1 >= *cap) {
        *cap = *cap ? *cap * 2 : 64;
        *arr = (Token*)realloc(*arr, (size_t)(*cap) * sizeof(Token));
    }
    (*arr)[(*cnt)++] = tk;
}

Token* lua_tokenize(const char* src, int* out_count, char* err, size_t err_cap) {
    Token* toks = NULL;
    int cnt = 0, cap = 0, line = 1;
    const char* p = src;
    if (p[0] == '#') while (*p && *p != '\n') p++; /* skip shebang */

    while (*p) {
        if (*p == '\n') { line++; p++; continue; }
        if (isspace((unsigned char)*p)) { p++; continue; }
        if (p[0] == '-' && p[1] == '-') {
            p += 2;
            int eq = 0, step = match_long_eq(p, &eq);
            if (step > 0) {
                p = read_long_bracket(p + step, eq, &line, NULL, NULL);
                if (!p) { snprintf(err, err_cap, "line %d: unfinished long comment", line); goto fail; }
            } else {
                while (*p && *p != '\n') p++;
            }
            continue;
        }
        Token tk; memset(&tk, 0, sizeof(tk)); tk.line = line;

        if (*p == '"' || *p == '\'') {
            p = read_string(p, *p, &line, &tk.text, &tk.len, err, err_cap);
            if (!p) goto fail;
            tk.kind = TK_STR;
            push_tok(&toks, &cnt, &cap, tk);
            continue;
        }
        int eq = 0, lstep = match_long_eq(p, &eq);
        if (lstep > 0) {
            p = read_long_bracket(p + lstep, eq, &line, &tk.text, &tk.len);
            if (!p) { snprintf(err, err_cap, "line %d: unfinished long string", line); goto fail; }
            tk.kind = TK_STR;
            push_tok(&toks, &cnt, &cap, tk);
            continue;
        }
        if (isdigit((unsigned char)*p) || (*p == '.' && isdigit((unsigned char)p[1]))) {
            const char* start = p;
            char* end = NULL;
            if (p[0] == '0' && (p[1] == 'x' || p[1] == 'X')) {
                tk.inum = strtoll(p, &end, 16);
                tk.is_int = 1;
                tk.num = (double)tk.inum;
            } else {
                double d = strtod(start, &end);
                int is_flt = 0;
                for (const char* q = start; q < end; q++) if (*q == '.' || *q == 'e' || *q == 'E') is_flt = 1;
                if (!is_flt) { tk.inum = strtoll(start, NULL, 10); tk.is_int = 1; tk.num = (double)tk.inum; }
                else { tk.num = d; tk.is_int = 0; }
            }
            p = end;
            tk.kind = TK_NUM;
            push_tok(&toks, &cnt, &cap, tk);
            continue;
        }
        if (isalpha((unsigned char)*p) || *p == '_') {
            const char* start = p;
            while (isalnum((unsigned char)*p) || *p == '_') p++;
            size_t len = (size_t)(p - start);
            tk.kind = TK_NAME;
            for (int i = 0; KEYWORDS[i].name; i++) {
                if (strlen(KEYWORDS[i].name) == len && memcmp(start, KEYWORDS[i].name, len) == 0) {
                    tk.kind = KEYWORDS[i].kind;
                    break;
                }
            }
            if (tk.kind == TK_NAME) {
                tk.text = (char*)malloc(len + 1);
                memcpy(tk.text, start, len);
                tk.text[len] = '\0';
                tk.len = len;
            }
            push_tok(&toks, &cnt, &cap, tk);
            continue;
        }

        if (p[0] == '.' && p[1] == '.' && p[2] == '.') { tk.kind = TK_DOTS; p += 3; }
        else if (p[0] == '.' && p[1] == '.') { tk.kind = TK_CONCAT; p += 2; }
        else if (p[0] == '/' && p[1] == '/') { tk.kind = TK_IDIV; p += 2; }
        else if (p[0] == '=' && p[1] == '=') { tk.kind = TK_EQ; p += 2; }
        else if (p[0] == '~' && p[1] == '=') { tk.kind = TK_NE; p += 2; }
        else if (p[0] == '<' && p[1] == '=') { tk.kind = TK_LE; p += 2; }
        else if (p[0] == '>' && p[1] == '=') { tk.kind = TK_GE; p += 2; }
        else {
            char c = *p++;
            switch (c) {
            case '+': tk.kind = TK_ADD; break; case '-': tk.kind = TK_SUB; break;
            case '*': tk.kind = TK_MUL; break; case '/': tk.kind = TK_DIV; break;
            case '%': tk.kind = TK_MOD; break; case '^': tk.kind = TK_POW; break;
            case '#': tk.kind = TK_LEN; break; case '<': tk.kind = TK_LT; break;
            case '>': tk.kind = TK_GT; break; case '=': tk.kind = TK_ASSIGN; break;
            case '(': tk.kind = TK_LPAREN; break; case ')': tk.kind = TK_RPAREN; break;
            case '{': tk.kind = TK_LBRACE; break; case '}': tk.kind = TK_RBRACE; break;
            case '[': tk.kind = TK_LBRACK; break; case ']': tk.kind = TK_RBRACK; break;
            case ';': tk.kind = TK_SEMI; break; case ':': tk.kind = TK_COLON; break;
            case ',': tk.kind = TK_COMMA; break; case '.': tk.kind = TK_DOT; break;
            default: snprintf(err, err_cap, "line %d: unexpected char '%c'", line, c); goto fail;
            }
        }
        push_tok(&toks, &cnt, &cap, tk);
    }

    Token eof; memset(&eof, 0, sizeof(eof)); eof.kind = TK_EOF; eof.line = line;
    push_tok(&toks, &cnt, &cap, eof);
    *out_count = cnt;
    return toks;
fail:
    lua_free_tokens(toks, cnt);
    *out_count = 0;
    return NULL;
}

void lua_free_tokens(Token* toks, int count) {
    if (!toks) return;
    for (int i = 0; i < count; i++) free(toks[i].text);
    free(toks);
}
