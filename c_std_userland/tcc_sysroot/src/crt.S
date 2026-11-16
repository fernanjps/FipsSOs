.text
.global _start
.type _start, @function
_start:
    xor %rbp, %rbp
    mov (%rsp), %rdi        /* argc */
    lea 8(%rsp), %rsi       /* argv */
    lea 16(%rsp,%rdi,8), %rdx /* envp */
    and $-16, %rsp
    call main
    mov %rax, %rdi
    call _exit
    hlt

.global __fips_syscall
.type __fips_syscall, @function
__fips_syscall:
    mov %rdi, %rax          /* syscall num */
    mov %rsi, %rdi          /* arg1 */
    mov %rdx, %rsi          /* arg2 */
    mov %rcx, %rdx          /* arg3 */
    mov %r8, %r10           /* arg4 */
    mov %r9, %r8            /* arg5 */
    mov 8(%rsp), %r9        /* arg6 */
    syscall
    ret
