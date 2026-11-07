use core::mem::size_of;
use core::sync::atomic::{AtomicU64, Ordering};

use super::gdt::{DOUBLE_FAULT_IST_INDEX, KERNEL_CODE_SELECTOR};

/// Interrupt vectors handled by FipSOs.
pub const DOUBLE_FAULT_VECTOR: usize = 8;
pub const GENERAL_PROTECTION_FAULT_VECTOR: usize = 13;
pub const PAGE_FAULT_VECTOR: usize = 14;
pub const TIMER_INTERRUPT_VECTOR: usize = 32; // IRQ0 remapped to 0x20
pub const KEYBOARD_INTERRUPT_VECTOR: usize = 33; // IRQ1 remapped to 0x21

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
    ".global __isr_double_fault",
    "__isr_double_fault:",
    "mov rsi, [rsp]",
    "lea rdi, [rsp + 8]",
    "and rsp, -16",
    "call __rust_double_fault_handler",

    ".global __isr_general_protection_fault",
    "__isr_general_protection_fault:",
    "mov rsi, [rsp]",
    "lea rdi, [rsp + 8]",
    "and rsp, -16",
    "call __rust_general_protection_fault_handler",

    ".global __isr_page_fault",
    "__isr_page_fault:",
    "mov rsi, [rsp]",
    "lea rdi, [rsp + 8]",
    "mov rdx, cr2",
    "and rsp, -16",
    "call __rust_page_fault_handler",

    ".global __isr_timer_irq",
    "__isr_timer_irq:",
    "push rax",
    "push rcx",
    "push rdx",
    "push rsi",
    "push rdi",
    "push r8",
    "push r9",
    "push r10",
    "push r11",
    "push rbp",
    "lea rdi, [rsp + 80]",
    "mov rbp, rsp",
    "and rsp, -16",
    "call __rust_timer_irq_handler",
    "mov rsp, rbp",
    "pop rbp",
    "pop r11",
    "pop r10",
    "pop r9",
    "pop r8",
    "pop rdi",
    "pop rsi",
    "pop rdx",
    "pop rcx",
    "pop rax",
    "iretq",

    ".global __isr_keyboard_irq",
    "__isr_keyboard_irq:",
    "push rax",
    "push rcx",
    "push rdx",
    "push rsi",
    "push rdi",
    "push r8",
    "push r9",
    "push r10",
    "push r11",
    "push rbp",
    "mov rbp, rsp",
    "and rsp, -16",
    "call __rust_keyboard_irq_handler",
    "mov rsp, rbp",
    "pop rbp",
    "pop r11",
    "pop r10",
    "pop r9",
    "pop r8",
    "pop rdi",
    "pop rsi",
    "pop rdx",
    "pop rcx",
    "pop rax",
    "iretq",

    ".global __isr_spurious_irq",
    "__isr_spurious_irq:",
    "iretq",
);

extern "C" {
    fn __isr_double_fault();
    fn __isr_general_protection_fault();
    fn __isr_page_fault();
    fn __isr_timer_irq();
    fn __isr_keyboard_irq();
    fn __isr_spurious_irq();
}

#[no_mangle]
extern "C" fn __rust_double_fault_handler(
    frame: &InterruptStackFrame,
    error_code: u64,
) -> ! {
    crate::early_println!(
        "\n[EXCEPTION #DF] Double Fault (error_code={:#x}) @ RIP={:#x}, RSP={:#x}",
        error_code,
        frame.instruction_pointer,
        frame.stack_pointer
    );
    crate::halt_loop()
}

#[no_mangle]
extern "C" fn __rust_general_protection_fault_handler(
    frame: &InterruptStackFrame,
    error_code: u64,
) -> ! {
    crate::early_println!(
        "\n[EXCEPTION #GP] General Protection Fault (error_code={:#x}) @ RIP={:#x}, CS={:#x}, RSP={:#x}",
        error_code,
        frame.instruction_pointer,
        frame.code_segment,
        frame.stack_pointer
    );
    crate::halt_loop()
}

#[no_mangle]
extern "C" fn __rust_page_fault_handler(
    frame: &InterruptStackFrame,
    error_code: u64,
    fault_addr: u64,
) -> ! {
    let present = (error_code & (1 << 0)) != 0;
    let write = (error_code & (1 << 1)) != 0;
    let user = (error_code & (1 << 2)) != 0;
    let instruction_fetch = (error_code & (1 << 4)) != 0;

    crate::early_println!(
        "\n[EXCEPTION #PF] Page Fault en CR2={:#x} (error_code={:#x}: present={}, write={}, user={}, ifetch={}) @ RIP={:#x}",
        fault_addr,
        error_code,
        present,
        write,
        user,
        instruction_fetch,
        frame.instruction_pointer
    );
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

/// Remaps the legacy 8259 PIC to vectors `0x20..0x2F` and configures PIT Channel 0.
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

        // Unmask IRQ0 (Timer) and IRQ1 (PS/2 Keyboard) on Master PIC (0xFC), mask all on Slave PIC (0xFF)
        outb(0x21, 0xFC);
        outb(0xA1, 0xFF);

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

/// Initializes the x86_64 IDT with handlers for Double Fault (`#DF`), General Protection Fault (`#GP`),
/// Page Fault (`#PF`), and the Timer Interrupt (`0x20`).
pub fn init() {
    // SAFETY: Populating static IDT and loading IDTR on the BSP during single-threaded boot.
    unsafe {
        for i in 32..256 {
            IDT.entries[i].set_handler(__isr_spurious_irq as *const () as usize, 0);
        }

        IDT.entries[DOUBLE_FAULT_VECTOR].set_handler(
            __isr_double_fault as *const () as usize,
            DOUBLE_FAULT_IST_INDEX as u8,
        );
        IDT.entries[GENERAL_PROTECTION_FAULT_VECTOR].set_handler(
            __isr_general_protection_fault as *const () as usize,
            0,
        );
        IDT.entries[PAGE_FAULT_VECTOR]
            .set_handler(__isr_page_fault as *const () as usize, 0);
        IDT.entries[TIMER_INTERRUPT_VECTOR]
            .set_handler(__isr_timer_irq as *const () as usize, 0);
        IDT.entries[KEYBOARD_INTERRUPT_VECTOR]
            .set_handler(__isr_keyboard_irq as *const () as usize, 0);

        let idtr = DescriptorTablePointer {
            limit: (size_of::<IdtTable>() - 1) as u16,
            base: (&raw const IDT) as u64,
        };

        core::arch::asm!("lidt [{idtr}]", idtr = in(reg) &idtr, options(preserves_flags));

        init_pic_and_pit();
        super::keyboard::init();
        // Trigger a self-test software interrupt on Timer Vector 0x20 to verify IDT dispatch
        core::arch::asm!("int 0x20", options(preserves_flags));
    }

    crate::early_println!(
        "[ostd::arch::idt] IDT cargada (#DF={}, #GP={}, #PF={}, Timer={:#x}, Keyboard IRQ1={:#x}, PIC mask=0xFC)",
        DOUBLE_FAULT_VECTOR,
        GENERAL_PROTECTION_FAULT_VECTOR,
        PAGE_FAULT_VECTOR,
        TIMER_INTERRUPT_VECTOR,
        KEYBOARD_INTERRUPT_VECTOR
    );
}

/// Returns the number of timer interrupt ticks recorded since boot.
#[inline]
pub fn timer_ticks() -> u64 {
    TIMER_TICKS.load(Ordering::Relaxed)
}
