use core::mem::size_of;

/// Selector index and privilege constants for the x86_64 GDT.
pub const KERNEL_CODE_SELECTOR: u16 = 0x08;
pub const KERNEL_DATA_SELECTOR: u16 = 0x10;
pub const USER_DATA_SELECTOR: u16 = 0x18 | 3;
pub const USER_CODE_SELECTOR: u16 = 0x20 | 3;
pub const TSS_SELECTOR: u16 = 0x28;

/// Interrupt Stack Table index (1-based) used for Double Faults (`#DF`).
pub const DOUBLE_FAULT_IST_INDEX: u16 = 1;

const STACK_SIZE: usize = 16 * 1024; // 16 KiB per stack

#[repr(C, align(16))]
struct AlignedStack([u8; STACK_SIZE]);

static mut RING0_STACK: AlignedStack = AlignedStack([0; STACK_SIZE]);
static mut DOUBLE_FAULT_STACK: AlignedStack = AlignedStack([0; STACK_SIZE]);

/// Architectural 64-bit Task State Segment (TSS).
#[repr(C, packed)]
pub struct TaskStateSegment {
    pub reserved_1: u32,
    pub privilege_stack_table: [u64; 3],
    pub reserved_2: u64,
    pub interrupt_stack_table: [u64; 7],
    pub reserved_3: u64,
    pub reserved_4: u16,
    pub iomap_base: u16,
}

impl TaskStateSegment {
    pub const fn new() -> Self {
        Self {
            reserved_1: 0,
            privilege_stack_table: [0; 3],
            reserved_2: 0,
            interrupt_stack_table: [0; 7],
            reserved_3: 0,
            reserved_4: 0,
            iomap_base: size_of::<Self>() as u16,
        }
    }
}

/// 7-slot Global Descriptor Table:
/// 0: Null
/// 1: Kernel Code (Ring 0, 64-bit)
/// 2: Kernel Data (Ring 0)
/// 3: User Data   (Ring 3)
/// 4: User Code   (Ring 3, 64-bit)
/// 5..6: 64-bit Task State Segment (TSS) System Descriptor (16 bytes)
#[repr(C, align(16))]
struct GdtTable {
    entries: [u64; 7],
}

#[repr(C, packed)]
struct DescriptorTablePointer {
    limit: u16,
    base: u64,
}

static mut TSS: TaskStateSegment = TaskStateSegment::new();
static mut GDT: GdtTable = GdtTable { entries: [0; 7] };

static mut AP_TSS: [TaskStateSegment; 4] = [
    TaskStateSegment::new(),
    TaskStateSegment::new(),
    TaskStateSegment::new(),
    TaskStateSegment::new(),
];
static mut AP_GDT: [GdtTable; 4] = [
    GdtTable { entries: [0; 7] },
    GdtTable { entries: [0; 7] },
    GdtTable { entries: [0; 7] },
    GdtTable { entries: [0; 7] },
];

/// Initializes the x86_64 GDT (Ring 0 & Ring 3 segments) and loads the TSS.
pub fn init() {
    // SAFETY: Executed once on the BSP during early boot before SMP or preemption.
    unsafe {
        let rsp0_top = (&raw const RING0_STACK).cast::<u8>().add(STACK_SIZE) as u64;
        let ist1_top = (&raw const DOUBLE_FAULT_STACK).cast::<u8>().add(STACK_SIZE) as u64;

        TSS.privilege_stack_table[0] = rsp0_top;
        TSS.interrupt_stack_table[(DOUBLE_FAULT_IST_INDEX - 1) as usize] = ist1_top;
        TSS.iomap_base = size_of::<TaskStateSegment>() as u16;

        let tss_base = (&raw const TSS) as u64;
        let tss_limit = (size_of::<TaskStateSegment>() - 1) as u64;

        let tss_low = (tss_limit & 0xFFFF)
            | ((tss_base & 0xFFFFFF) << 16)
            | (0x89u64 << 40) // Present=1, DPL=0, Type=0x9 (Available 64-bit TSS)
            | (((tss_limit >> 16) & 0xF) << 48)
            | (((tss_base >> 24) & 0xFF) << 56);
        let tss_high = tss_base >> 32;

        GDT.entries[0] = 0; // 0x00: Null descriptor
        GDT.entries[1] = 0x00AF_9A00_0000_FFFF; // 0x08: Kernel Code (Ring 0, L=1, DPL=0)
        GDT.entries[2] = 0x00CF_9200_0000_FFFF; // 0x10: Kernel Data (Ring 0, W=1, DPL=0)
        GDT.entries[3] = 0x00CF_F200_0000_FFFF; // 0x18: User Data   (Ring 3, W=1, DPL=3)
        GDT.entries[4] = 0x00AF_FA00_0000_FFFF; // 0x20: User Code   (Ring 3, L=1, DPL=3)
        GDT.entries[5] = tss_low; // 0x28: TSS Descriptor (low 64 bits)
        GDT.entries[6] = tss_high; // 0x30: TSS Descriptor (high 64 bits)

        let gdtr = DescriptorTablePointer {
            limit: (size_of::<GdtTable>() - 1) as u16,
            base: (&raw const GDT) as u64,
        };

        core::arch::asm!(
            "lgdt [{gdtr}]",
            // Reload CS with KERNEL_CODE_SELECTOR (0x08) via far return
            "push {kcode}",
            "lea {tmp}, [rip + 2f]",
            "push {tmp}",
            "retfq",
            "2:",
            // Reload data/stack segment selectors with KERNEL_DATA_SELECTOR (0x10)
            "mov ax, {kdata:x}",
            "mov ds, ax",
            "mov es, ax",
            "mov ss, ax",
            // Load Task State Segment selector (0x28)
            "ltr {tss:x}",
            gdtr = in(reg) &gdtr,
            kcode = const KERNEL_CODE_SELECTOR as u64,
            kdata = in(reg) KERNEL_DATA_SELECTOR,
            tss = in(reg) TSS_SELECTOR,
            tmp = lateout(reg) _,
            out("ax") _,
            options(preserves_flags)
        );
    }

    crate::early_println!(
        "[ostd::arch::gdt] GDT (Ring 0: {:#04x}/{:#04x}, Ring 3: {:#04x}/{:#04x}) y TSS ({:#04x}, IST1 Activo) cargados",
        KERNEL_CODE_SELECTOR,
        KERNEL_DATA_SELECTOR,
        USER_CODE_SELECTOR,
        USER_DATA_SELECTOR,
        TSS_SELECTOR
    );
}

/// Initializes a dedicated GDT and TSS for an Application Processor (`AP`),
/// loading `GDTR`, resetting segment registers (`CS=0x08`, `DS/ES/SS=0x10`), and loading `TR=0x28`.
pub unsafe fn init_ap_gdt(cpu_id: usize, stack_top: u64) {
    let idx = cpu_id.min(3);
    let tss = &raw mut AP_TSS[idx];
    (*tss).privilege_stack_table[0] = stack_top;
    (*tss).interrupt_stack_table[(DOUBLE_FAULT_IST_INDEX - 1) as usize] = stack_top;
    (*tss).iomap_base = size_of::<TaskStateSegment>() as u16;

    let tss_base = tss as u64;
    let tss_limit = (size_of::<TaskStateSegment>() - 1) as u64;

    let tss_low = (tss_limit & 0xFFFF)
        | ((tss_base & 0xFFFFFF) << 16)
        | (0x89u64 << 40) // Present=1, DPL=0, Type=0x9 (Available 64-bit TSS)
        | (((tss_limit >> 16) & 0xF) << 48)
        | (((tss_base >> 24) & 0xFF) << 56);
    let tss_high = tss_base >> 32;

    let gdt = &raw mut AP_GDT[idx];
    (*gdt).entries[0] = 0;
    (*gdt).entries[1] = 0x00AF_9A00_0000_FFFF; // 0x08: Kernel Code
    (*gdt).entries[2] = 0x00CF_9200_0000_FFFF; // 0x10: Kernel Data
    (*gdt).entries[3] = 0x00CF_F200_0000_FFFF; // 0x18: User Data
    (*gdt).entries[4] = 0x00AF_FA00_0000_FFFF; // 0x20: User Code
    (*gdt).entries[5] = tss_low;              // 0x28: TSS low
    (*gdt).entries[6] = tss_high;             // 0x30: TSS high

    let gdtr = DescriptorTablePointer {
        limit: (size_of::<GdtTable>() - 1) as u16,
        base: gdt as u64,
    };

    core::arch::asm!(
        "lgdt [{gdtr}]",
        "push {kcode}",
        "lea {tmp}, [rip + 2f]",
        "push {tmp}",
        "retfq",
        "2:",
        "mov ax, {kdata:x}",
        "mov ds, ax",
        "mov es, ax",
        "mov ss, ax",
        "ltr {tss:x}",
        gdtr = in(reg) &gdtr,
        kcode = const KERNEL_CODE_SELECTOR as u64,
        kdata = in(reg) KERNEL_DATA_SELECTOR,
        tss = in(reg) TSS_SELECTOR,
        tmp = lateout(reg) _,
        out("ax") _,
        options(preserves_flags)
    );
}
