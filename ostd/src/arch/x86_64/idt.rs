use core::fmt::Write;
use core::mem::size_of;
use core::sync::atomic::{AtomicU64, Ordering};

use super::gdt::{DOUBLE_FAULT_IST_INDEX, KERNEL_CODE_SELECTOR};

/// Interrupt vectors handled by FipSOs.
pub const DOUBLE_FAULT_VECTOR: usize = 8;
pub const GENERAL_PROTECTION_FAULT_VECTOR: usize = 13;
pub const PAGE_FAULT_VECTOR: usize = 14;
pub const TIMER_INTERRUPT_VECTOR: usize = 32; // IRQ0 remapped to 0x20
pub const KEYBOARD_INTERRUPT_VECTOR: usize = 33; // IRQ1 remapped to 0x21
pub const MOUSE_INTERRUPT_VECTOR: usize = 44; // IRQ12 remapped to 0x2C (Slave PIC line 4)

static TIMER_TICKS: AtomicU64 = AtomicU64::new(0);

/// Hardware exception stack frame pushed by the x86_64 CPU on interrupt/exception entry.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct InterruptStackFrame {
    pub instruction_pointer: u64,
    pub code_segment: u64,
    pub cpu_flags: u64,
    pub stack_pointer: u64,
    pub stack_segment: u64,
}

/// 16-byte IDT Gate Descriptor for 64-bit Long Mode.
#[repr(C)]
#[derive(Copy, Clone)]
struct IdtEntry {
    offset_low: u16,
    selector: u16,
    ist: u8,
    type_attributes: u8,
    offset_mid: u16,
    offset_high: u32,
    reserved: u32,
}

impl IdtEntry {
    const fn missing() -> Self {
        Self {
            offset_low: 0,
            selector: 0,
            ist: 0,
            type_attributes: 0,
            offset_mid: 0,
            offset_high: 0,
            reserved: 0,
        }
    }

    fn set_handler(&mut self, handler_addr: usize, ist_index: u8) {
        self.offset_low = handler_addr as u16;
        self.selector = KERNEL_CODE_SELECTOR;
        self.ist = ist_index & 0x7;
        // Present=1, DPL=00, Type=0xE (64-bit Interrupt Gate, clears IF on entry)
        self.type_attributes = 0x8E;
        self.offset_mid = (handler_addr >> 16) as u16;
        self.offset_high = (handler_addr >> 32) as u32;
        self.reserved = 0;
    }

    fn set_handler_dpl3(&mut self, handler_addr: usize, ist_index: u8) {
        self.set_handler(handler_addr, ist_index);
        // Present=1, DPL=3, Type=0xE (Allows Ring 3 software traps e.g. INT3)
        self.type_attributes = 0xEE;
    }
}

#[repr(C, align(16))]
struct IdtTable {
    entries: [IdtEntry; 256],
}

#[repr(C, packed)]
struct DescriptorTablePointer {
    limit: u16,
    base: u64,
}

static mut IDT: IdtTable = IdtTable {
    entries: [IdtEntry::missing(); 256],
};

core::arch::global_asm!(
    ".macro ISR_NO_ERR vec",
    ".global __isr_vec_\\vec",
    "__isr_vec_\\vec:",
    "    push 0",
    "    push \\vec",
    "    jmp __isr_common_exception",
    ".endm",

    ".macro ISR_ERR vec",
    ".global __isr_vec_\\vec",
    "__isr_vec_\\vec:",
    "    push \\vec",
    "    jmp __isr_common_exception",
    ".endm",

    "ISR_NO_ERR 0\n ISR_NO_ERR 1\n ISR_NO_ERR 2\n ISR_NO_ERR 3\n ISR_NO_ERR 4\n ISR_NO_ERR 5\n ISR_NO_ERR 6\n ISR_NO_ERR 7",
    "ISR_ERR    8\n ISR_NO_ERR 9\n ISR_ERR 10\n ISR_ERR 11\n ISR_ERR 12\n ISR_ERR 13\n ISR_ERR 14\n ISR_NO_ERR 15",
    "ISR_NO_ERR 16\n ISR_ERR 17\n ISR_NO_ERR 18\n ISR_NO_ERR 19\n ISR_NO_ERR 20\n ISR_ERR 21\n ISR_NO_ERR 22\n ISR_NO_ERR 23",
    "ISR_NO_ERR 24\n ISR_NO_ERR 25\n ISR_NO_ERR 26\n ISR_NO_ERR 27\n ISR_NO_ERR 28\n ISR_ERR 29\n ISR_ERR 30\n ISR_NO_ERR 31",

    ".global __isr_common_exception",
    "__isr_common_exception:",
    "    push r15",
    "    push r14",
    "    push r13",
    "    push r12",
    "    push rbp",
    "    push rbx",
    "    push r11",
    "    push r10",
    "    push r9",
    "    push r8",
    "    push rdi",
    "    push rsi",
    "    push rdx",
    "    push rcx",
    "    push rax",

    "    test byte ptr [rsp + 144], 3", /* Check CS RPL (bit 0..1): was it Ring 3? */
    "    jz .Lexc_kentry",
    "    swapgs",
    "    mov ax, 0x10",
    "    mov ds, ax",
    "    mov es, ax",
    ".Lexc_kentry:",
    "    lea rdi, [rsp + 136]", /* &InterruptStackFrame (rip, cs, rflags, rsp, ss) */
    "    mov rsi, [rsp + 128]", /* error_code */
    "    mov rdx, cr2",         /* cr2 */
    "    mov rcx, [rsp + 120]", /* vector */

    "    mov rbp, rsp",
    "    and rsp, -16",
    "    call __rust_exception_dispatcher",
    "    mov rsp, rbp",

    "    test byte ptr [rsp + 144], 3",
    "    jz .Lexc_kexit",
    "    mov ax, 0x1b",
    "    mov ds, ax",
    "    mov es, ax",
    "    swapgs",
    ".Lexc_kexit:",
    "    pop rax",
    "    pop rcx",
    "    pop rdx",
    "    pop rsi",
    "    pop rdi",
    "    pop r8",
    "    pop r9",
    "    pop r10",
    "    pop r11",
    "    pop rbx",
    "    pop rbp",
    "    pop r12",
    "    pop r13",
    "    pop r14",
    "    pop r15",
    "    add rsp, 16",          /* vector + error_code */
    "    iretq",

    ".global __isr_timer_irq",
    "__isr_timer_irq:",
    "    push rax",
    "    push rcx",
    "    push rdx",
    "    push rsi",
    "    push rdi",
    "    push r8",
    "    push r9",
    "    push r10",
    "    push r11",
    "    push rbp",
    "    mov rbp, rsp",

    "    test byte ptr [rbp + 88], 3", /* CS is at [rbp + 88] */
    "    jz .Ltimer_kentry",
    "    swapgs",
    "    mov ax, 0x10",
    "    mov ds, ax",
    "    mov es, ax",
    ".Ltimer_kentry:",
    "    sub rsp, 512",
    "    and rsp, -16",
    "    fxsave64 [rsp]",
    "    lea rdi, [rbp + 80]",
    "    call __rust_timer_irq_handler",
    "    fxrstor64 [rsp]",
    "    mov rsp, rbp",

    "    test byte ptr [rbp + 88], 3",
    "    jz .Ltimer_kexit",
    "    mov ax, 0x1b",
    "    mov ds, ax",
    "    mov es, ax",
    "    swapgs",
    ".Ltimer_kexit:",
    "    pop rbp",
    "    pop r11",
    "    pop r10",
    "    pop r9",
    "    pop r8",
    "    pop rdi",
    "    pop rsi",
    "    pop rdx",
    "    pop rcx",
    "    pop rax",
    "    iretq",

    ".global __isr_keyboard_irq",
    "__isr_keyboard_irq:",
    "    push rax",
    "    push rcx",
    "    push rdx",
    "    push rsi",
    "    push rdi",
    "    push r8",
    "    push r9",
    "    push r10",
    "    push r11",
    "    push rbp",
    "    mov rbp, rsp",

    "    test byte ptr [rbp + 88], 3",
    "    jz .Lkb_kentry",
    "    swapgs",
    "    mov ax, 0x10",
    "    mov ds, ax",
    "    mov es, ax",
    ".Lkb_kentry:",
    "    sub rsp, 512",
    "    and rsp, -16",
    "    fxsave64 [rsp]",
    "    call __rust_keyboard_irq_handler",
    "    fxrstor64 [rsp]",
    "    mov rsp, rbp",

    "    test byte ptr [rbp + 88], 3",
    "    jz .Lkb_kexit",
    "    mov ax, 0x1b",
    "    mov ds, ax",
    "    mov es, ax",
    "    swapgs",
    ".Lkb_kexit:",
    "    pop rbp",
    "    pop r11",
    "    pop r10",
    "    pop r9",
    "    pop r8",
    "    pop rdi",
    "    pop rsi",
    "    pop rdx",
    "    pop rcx",
    "    pop rax",
    "    iretq",

    ".global __isr_mouse_irq",
    "__isr_mouse_irq:",
    "    push rax",
    "    push rcx",
    "    push rdx",
    "    push rsi",
    "    push rdi",
    "    push r8",
    "    push r9",
    "    push r10",
    "    push r11",
    "    push rbp",
    "    mov rbp, rsp",

    "    test byte ptr [rbp + 88], 3",
    "    jz .Lmouse_kentry",
    "    swapgs",
    "    mov ax, 0x10",
    "    mov ds, ax",
    "    mov es, ax",
    ".Lmouse_kentry:",
    "    sub rsp, 512",
    "    and rsp, -16",
    "    fxsave64 [rsp]",
    "    call __rust_mouse_irq_handler",
    "    fxrstor64 [rsp]",
    "    mov rsp, rbp",

    "    test byte ptr [rbp + 88], 3",
    "    jz .Lmouse_kexit",
    "    mov ax, 0x1b",
    "    mov ds, ax",
    "    mov es, ax",
    "    swapgs",
    ".Lmouse_kexit:",
    "    pop rbp",
    "    pop r11",
    "    pop r10",
    "    pop r9",
    "    pop r8",
    "    pop rdi",
    "    pop rsi",
    "    pop rdx",
    "    pop rcx",
    "    pop rax",
    "    iretq",

    ".global __isr_spurious_irq",
    "__isr_spurious_irq:",
    "    iretq",
);

extern "C" {
    fn __isr_vec_0(); fn __isr_vec_1(); fn __isr_vec_2(); fn __isr_vec_3();
    fn __isr_vec_4(); fn __isr_vec_5(); fn __isr_vec_6(); fn __isr_vec_7();
    fn __isr_vec_8(); fn __isr_vec_9(); fn __isr_vec_10(); fn __isr_vec_11();
    fn __isr_vec_12(); fn __isr_vec_13(); fn __isr_vec_14(); fn __isr_vec_15();
    fn __isr_vec_16(); fn __isr_vec_17(); fn __isr_vec_18(); fn __isr_vec_19();
    fn __isr_vec_20(); fn __isr_vec_21(); fn __isr_vec_22(); fn __isr_vec_23();
    fn __isr_vec_24(); fn __isr_vec_25(); fn __isr_vec_26(); fn __isr_vec_27();
    fn __isr_vec_28(); fn __isr_vec_29(); fn __isr_vec_30(); fn __isr_vec_31();

    fn __isr_timer_irq();
    fn __isr_keyboard_irq();
    fn __isr_mouse_irq();
    fn __isr_spurious_irq();

    fn c_lookup_exception_mnemonic(vector: u64) -> *const core::ffi::c_char;
    fn c_lookup_exception_name(vector: u64) -> *const core::ffi::c_char;
    fn c_lookup_posix_signal(vector: u64) -> i32;
    fn c_format_panic_diagnostic(
        buf: *mut core::ffi::c_char,
        max_len: usize,
        vector: u64,
        error_code: u64,
        cr2: u64,
        rip: u64,
        cs: u64,
        rflags: u64,
        rsp: u64,
    ) -> usize;
}

#[no_mangle]
extern "C" fn __rust_exception_dispatcher(
    frame: &InterruptStackFrame,
    error_code: u64,
    fault_addr: u64,
    vector: u64,
) {
    let is_user_fault = (frame.code_segment & 0b11) == 3 || (vector == 14 && (error_code & 4) != 0);

    if is_user_fault {
        // Fault occurred in Ring 3 User Space: Kernel GS was already restored conditionally by the ASM stub
        let sig = unsafe { c_lookup_posix_signal(vector) };
        super::syscall::recover_from_user_fault(frame.instruction_pointer, fault_addr, sig);
    }

    // Unhandled exception in Ring 0 Kernel Space: format full diagnostic via C decoder and panic
    let mut diag_buf = [0u8; 512];
    let len = unsafe {
        c_format_panic_diagnostic(
            diag_buf.as_mut_ptr() as *mut core::ffi::c_char,
            diag_buf.len(),
            vector,
            error_code,
            fault_addr,
            frame.instruction_pointer,
            frame.code_segment,
            frame.cpu_flags,
            frame.stack_pointer,
        )
    };
    let diag_str = core::str::from_utf8(&diag_buf[..len]).unwrap_or("[KERNEL PANIC: Diagnostic format error]");

    crate::early_println!("\n{}", diag_str);
    crate::fb_println!("\n{}", diag_str);

    let _ = crate::fb_console::with_console(|c| {
        let _ = c.write_str("\n\x1b[1;31m======================================================================\x1b[0m\n");
        let _ = c.write_str("\x1b[1;41;37m [KERNEL PANIC: UNRECOVERABLE RING 0 EXCEPTION]                      \x1b[0m\n");
        let _ = c.write_str("\x1b[1;31m======================================================================\x1b[0m\n");
        let _ = c.write_str(diag_str);
        let _ = c.write_str("\x1b[1;31m======================================================================\x1b[0m\n");
        let _ = c.write_str("System halted. Please reboot the machine.\n");
    });

    crate::halt_loop()
}

#[no_mangle]
extern "C" fn __rust_timer_irq_handler(frame: &InterruptStackFrame) {
    let ticks = TIMER_TICKS.fetch_add(1, Ordering::Relaxed) + 1;
    crate::scheduler::on_timer_irq(frame, ticks);
    // Send End-Of-Interrupt (EOI = 0x20) to Master 8259 PIC (port 0x20)
    // SAFETY: Standard 8259 PIC EOI command on port 0x20.
    unsafe {
        core::arch::asm!(
            "out 0x20, al",
            in("al") 0x20u8,
            options(nomem, nostack, preserves_flags)
        );
    }
}

#[no_mangle]
extern "C" fn __rust_keyboard_irq_handler() {
    super::keyboard::on_irq1();
    // Send End-Of-Interrupt (EOI = 0x20) to Master 8259 PIC (port 0x20)
    // SAFETY: Standard 8259 PIC EOI command on port 0x20.
    unsafe {
        core::arch::asm!(
            "out 0x20, al",
            in("al") 0x20u8,
            options(nomem, nostack, preserves_flags)
        );
    }
}

#[no_mangle]
extern "C" fn __rust_mouse_irq_handler() {
    super::mouse::on_irq12();
    // Send End-Of-Interrupt (EOI = 0x20) to BOTH Slave 8259 PIC (0xA0) and Master 8259 PIC (0x20)
    // SAFETY: Standard cascaded 8259 PIC EOI sequence for IRQ8..IRQ15 in Ring 0.
    unsafe {
        outb(0xA0, 0x20);
        outb(0x20, 0x20);
    }
}

/// Remaps the legacy 8259 PIC to vectors `0x20..0x2F`, unmasks `IRQ0` (Timer), `IRQ1` (Keyboard),
/// `IRQ2` (Slave Cascade), and `IRQ12` (PS/2 Mouse), and configures PIT Channel 0 (~100 Hz).
unsafe fn init_pic_and_pit() {
    // SAFETY: Standard x86 8259 PIC & 8254 PIT I/O port initialization in Ring 0.
    unsafe {
        // ICW1: start initialization in cascade mode
        outb(0x20, 0x11);
        outb(0xA0, 0x11);
        // ICW2: Master PIC vector offset 0x20 (32), Slave PIC vector offset 0x28 (40)
        outb(0x21, 0x20);
        outb(0xA1, 0x28);
        // ICW3: tell Master PIC about Slave PIC at IRQ2 (0x04), tell Slave its cascade identity (0x02)
        outb(0x21, 0x04);
        outb(0xA1, 0x02);
        // ICW4: 8086/88 mode
        outb(0x21, 0x01);
        outb(0xA1, 0x01);

        // Master PIC (0x21) mask = 0xF8: allow IRQ0 (Timer), IRQ1 (Keyboard), IRQ2 (Cascade)
        // Slave PIC  (0xA1) mask = 0xEF: allow IRQ12 (line 4 on Slave PIC = PS/2 Mouse)
        outb(0x21, 0xF8);
        outb(0xA1, 0xEF);

        // Configure 8254 PIT Channel 0: rate generator (mode 2), ~100 Hz (divisor = 11931 = 0x2E9B)
        outb(0x43, 0x34);
        outb(0x40, 0x9B);
        outb(0x40, 0x2E);
    }
}

#[inline]
unsafe fn outb(port: u16, value: u8) {
    // SAFETY: Caller guarantees valid I/O port.
    unsafe {
        core::arch::asm!(
            "out dx, al",
            in("dx") port,
            in("al") value,
            options(nomem, nostack, preserves_flags)
        );
    }
}

/// Initializes the x86_64 IDT with all 32 hardware exception vectors (`0..31`),
/// hardware IRQs (Timer 0x20, Keyboard 0x21, Mouse 0x2C), and spurious defaults.
pub fn init() {
    // SAFETY: Populating static IDT and loading IDTR on the BSP during single-threaded boot.
    unsafe {
        for i in 32..256 {
            IDT.entries[i].set_handler(__isr_spurious_irq as *const () as usize, 0);
        }

        let exception_stubs: [unsafe extern "C" fn(); 32] = [
            __isr_vec_0,  __isr_vec_1,  __isr_vec_2,  __isr_vec_3,
            __isr_vec_4,  __isr_vec_5,  __isr_vec_6,  __isr_vec_7,
            __isr_vec_8,  __isr_vec_9,  __isr_vec_10, __isr_vec_11,
            __isr_vec_12, __isr_vec_13, __isr_vec_14, __isr_vec_15,
            __isr_vec_16, __isr_vec_17, __isr_vec_18, __isr_vec_19,
            __isr_vec_20, __isr_vec_21, __isr_vec_22, __isr_vec_23,
            __isr_vec_24, __isr_vec_25, __isr_vec_26, __isr_vec_27,
            __isr_vec_28, __isr_vec_29, __isr_vec_30, __isr_vec_31,
        ];

        for (vec, stub) in exception_stubs.iter().enumerate() {
            let ist = if vec == DOUBLE_FAULT_VECTOR {
                DOUBLE_FAULT_IST_INDEX as u8
            } else {
                0
            };
            if vec == 3 {
                // Vector 3 (#BP Breakpoint) allows Ring 3 invocation via INT3
                IDT.entries[vec].set_handler_dpl3(*stub as *const () as usize, ist);
            } else {
                IDT.entries[vec].set_handler(*stub as *const () as usize, ist);
            }
        }

        IDT.entries[TIMER_INTERRUPT_VECTOR]
            .set_handler(__isr_timer_irq as *const () as usize, 0);
        IDT.entries[KEYBOARD_INTERRUPT_VECTOR]
            .set_handler(__isr_keyboard_irq as *const () as usize, 0);
        IDT.entries[MOUSE_INTERRUPT_VECTOR]
            .set_handler(__isr_mouse_irq as *const () as usize, 0);

        let idtr = DescriptorTablePointer {
            limit: (size_of::<IdtTable>() - 1) as u16,
            base: (&raw const IDT) as u64,
        };

        core::arch::asm!("lidt [{idtr}]", idtr = in(reg) &idtr, options(preserves_flags));

        super::keyboard::init();
        super::mouse::init();
        init_pic_and_pit();
        // Trigger a self-test software interrupt on Timer Vector 0x20 to verify IDT dispatch
        core::arch::asm!("int 0x20", options(preserves_flags));
    }

    crate::early_println!(
        "[ostd::arch::idt] IDT cargada: 32 vectores de excepcion blindados (0..31) | IRQ0={:#x}, IRQ1={:#x}, IRQ12={:#x}",
        TIMER_INTERRUPT_VECTOR,
        KEYBOARD_INTERRUPT_VECTOR,
        MOUSE_INTERRUPT_VECTOR
    );
    crate::fb_println!(
        "[ostd::arch::idt] IDT blindada: 32 vectores (0..31) activos | C11 panic_decoder vinculado"
    );
}

/// Returns the architectural mnemonic string for an x86_64 CPU exception vector (e.g. `"#PF"`).
pub fn exception_mnemonic(vector: u64) -> &'static str {
    let ptr = unsafe { c_lookup_exception_mnemonic(vector) };
    if ptr.is_null() {
        return "#UNK";
    }
    unsafe { core::ffi::CStr::from_ptr(ptr) }.to_str().unwrap_or("#UNK")
}

/// Returns the human-readable name of an architectural x86_64 CPU exception vector (e.g. `"Page Fault"`).
pub fn exception_name(vector: u64) -> &'static str {
    let ptr = unsafe { c_lookup_exception_name(vector) };
    if ptr.is_null() {
        return "Unknown Exception";
    }
    unsafe { core::ffi::CStr::from_ptr(ptr) }.to_str().unwrap_or("Unknown Exception")
}

/// Returns the number of timer interrupt ticks recorded since boot.
#[inline]
pub fn timer_ticks() -> u64 {
    TIMER_TICKS.load(Ordering::Relaxed)
}

/// Reloads the IDTR register on an Application Processor (`AP`).
pub fn load_idtr() {
    // SAFETY: Loading IDTR with the initialized kernel IDT in Ring 0.
    unsafe {
        let idtr = DescriptorTablePointer {
            limit: (size_of::<IdtTable>() - 1) as u16,
            base: (&raw const IDT) as u64,
        };
        core::arch::asm!("lidt [{idtr}]", idtr = in(reg) &idtr, options(preserves_flags));
    }
}
