.section .text
.code64

# ==============================================================================
# __fipsos_context_switch(prev_rsp: *mut u64 [rdi], next_rsp: u64 [rsi])
# ==============================================================================
# Real hardware CPU context switch in pure x86_64 assembly (System V AMD64 ABI):
# 1. Saves RFLAGS (`pushfq`) and all callee-saved registers (`rbp`, `rbx`,
#    `r12`, `r13`, `r14`, `r15`) onto the current thread's stack.
# 2. Stores the current stack pointer `rsp` into `[rdi]` (`*prev_rsp = rsp`).
# 3. Switches the hardware stack pointer `rsp` to `rsi` (`rsp = next_rsp`).
# 4. Restores callee-saved registers (`r15`..`r12`, `rbx`, `rbp`) and RFLAGS
#    (`popfq`) from the new stack and jumps to the target thread's `RIP` via `ret`.
.global __fipsos_context_switch
__fipsos_context_switch:
    pushfq
    push rbp
    push rbx
    push r12
    push r13
    push r14
    push r15

    mov [rdi], rsp
    mov rsp, rsi

    pop r15
    pop r14
    pop r13
    pop r12
    pop rbx
    pop rbp
    popfq
    ret
