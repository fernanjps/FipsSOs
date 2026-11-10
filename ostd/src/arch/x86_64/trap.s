.section .text

.global __fipsos_syscall_entry
__fipsos_syscall_entry:
    /* Swap User GS with Kernel GS (IA32_KERNEL_GS_BASE -> CpuLocal) */
    swapgs

    /* Save Ring 3 user RSP to CpuLocal.user_rsp_scratch (gs:[0x08])
       and load Ring 0 kernel stack from CpuLocal.kernel_syscall_rsp (gs:[0x00]) */
    mov gs:[0x08], rsp
    mov rsp, gs:[0x00]

    /* Construct TrapFrame (16 * 8 = 128 bytes) on the kernel stack */
    push qword ptr gs:[0x08] /* user_rsp */
    push r11                 /* user_rflags */
    push rcx                 /* user_rip */
    push r15
    push r14
    push r13
    push r12
    push rbp
    push rbx
    push r9                  /* arg5 */
    push r8                  /* arg4 */
    push r10                 /* arg3 */
    push rdx                 /* arg2 */
    push rsi                 /* arg1 */
    push rdi                 /* arg0 */
    push rax                 /* syscall_num */

    /* Ensure Kernel Data Segment selectors are active in Ring 0 */
    mov ax, 0x10
    mov ds, ax
    mov es, ax
    mov ss, ax

    /* Pass &mut TrapFrame in RDI to Rust dispatcher */
    mov rdi, rsp
    call __ostd_syscall_dispatch

    /* If dispatcher returns non-zero in RAX, exit Ring 3 back to kernel caller */
    test rax, rax
    jnz __fipsos_usermode_exit

    /* Restore TrapFrame and return to Ring 3 via swapgs + sysretq */
    mov ax, 0x1b
    mov ds, ax
    mov es, ax

    pop rax
    pop rdi
    pop rsi
    pop rdx
    pop r10
    pop r8
    pop r9
    pop rbx
    pop rbp
    pop r12
    pop r13
    pop r14
    pop r15
    pop rcx
    pop r11
    cli
    pop rsp

    swapgs
    sysretq

.global __fipsos_enter_ring3
__fipsos_enter_ring3:
    /* rdi = user_rip, rsi = user_rsp */
    push rbp
    push rbx
    push r12
    push r13
    push r14
    push r15

    /* Save kernel RSP in CpuLocal.saved_kernel_rsp (gs:[0x10]) */
    mov gs:[0x10], rsp

    mov ax, 0x1b
    mov ds, ax
    mov es, ax

    /* Build iretq frame for Ring 3 transition */
    push 0x1b          /* SS = USER_DATA_SELECTOR (0x18 | RPL 3) */
    push rsi           /* RSP = user_rsp */
    push 0x202         /* RFLAGS (bit 9 IF = 1 interrupts enabled, bit 1 reserved = 1) */
    push 0x23          /* CS = USER_CODE_SELECTOR (0x20 | RPL 3) */
    push rdi           /* RIP = user_rip */

    xor rax, rax
    xor rbx, rbx
    xor rcx, rcx
    xor rdx, rdx
    xor rsi, rsi
    xor rdi, rdi
    xor rbp, rbp
    xor r8, r8
    xor r9, r9
    xor r10, r10
    xor r11, r11
    xor r12, r12
    xor r13, r13
    xor r14, r14
    xor r15, r15

    /* Swap Kernel GS into IA32_KERNEL_GS_BASE before entering Ring 3 */
    swapgs
    iretq

.global __fipsos_usermode_exit
__fipsos_usermode_exit:
    /* Note: swapgs was already executed at __fipsos_syscall_entry, so GS points to CpuLocal */
    mov ax, 0x10
    mov ds, ax
    mov es, ax

    mov rsp, gs:[0x10]
    mov rax, gs:[0x18]

    pop r15
    pop r14
    pop r13
    pop r12
    pop rbx
    pop rbp
    ret

.global __fipsos_resume_from_fault
__fipsos_resume_from_fault:
    /* RDI = const TrapFrame* */
    /* Set up user data segment selectors for Ring 3 */
    mov ax, 0x1b
    mov ds, ax
    mov es, ax

    /* Build iretq stack frame */
    push 0x1b                   /* SS = 0x18 | RPL 3 */
    push qword ptr [rdi + 120]  /* user_rsp */
    push qword ptr [rdi + 112]  /* user_rflags */
    push 0x23                   /* CS = 0x20 | RPL 3 */
    push qword ptr [rdi + 104]  /* user_rip */

    /* Restore GPRs from TrapFrame */
    mov rax, [rdi + 0]
    mov rsi, [rdi + 16]
    mov rdx, [rdi + 24]
    mov r10, [rdi + 32]
    mov r8,  [rdi + 40]
    mov r9,  [rdi + 48]
    mov rbx, [rdi + 56]
    mov rbp, [rdi + 64]
    mov r12, [rdi + 72]
    mov r13, [rdi + 80]
    mov r14, [rdi + 88]
    mov r15, [rdi + 96]
    mov rdi, [rdi + 8]          /* restore rdi last */

    /* Swap kernel GS out so user GS is active */
    swapgs
    iretq

