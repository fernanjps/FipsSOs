//! Symmetric Multi-Processing (SMP) & AP Bootstrapping (`ostd::arch::x86_64::smp`).
//!
//! Conforms to the Limine MP / SMP Boot Protocol:
//! - Enumerates all Application Processors (APs) reported by ACPI / Firmware.
//! - Awakes each AP via `goto_address`.
//! - Configures per-core page tables (`MASTER_KERNEL_PML4`), dedicated 16 KiB AP stack,
//!   per-core GDT/TSS, IDT, FPU/SSE2, and Local APIC (xAPIC).
//! - Enters the concurrent task execution loop in `ostd::scheduler`.


use core::sync::atomic::{AtomicU64, Ordering};

/// Dedicated 16 KiB stack per Application Processor (AP).
pub const AP_STACK_SIZE: usize = 16 * 1024;

static LAPIC_VIRT: AtomicU64 = AtomicU64::new(0);

#[repr(C, align(16))]
struct ApStack([u8; AP_STACK_SIZE]);

static mut AP_STACKS: [ApStack; 16] = [
    ApStack([0; AP_STACK_SIZE]),
    ApStack([0; AP_STACK_SIZE]),
    ApStack([0; AP_STACK_SIZE]),
    ApStack([0; AP_STACK_SIZE]),
    ApStack([0; AP_STACK_SIZE]),
    ApStack([0; AP_STACK_SIZE]),
    ApStack([0; AP_STACK_SIZE]),
    ApStack([0; AP_STACK_SIZE]),
    ApStack([0; AP_STACK_SIZE]),
    ApStack([0; AP_STACK_SIZE]),
    ApStack([0; AP_STACK_SIZE]),
    ApStack([0; AP_STACK_SIZE]),
    ApStack([0; AP_STACK_SIZE]),
    ApStack([0; AP_STACK_SIZE]),
    ApStack([0; AP_STACK_SIZE]),
    ApStack([0; AP_STACK_SIZE]),
];

const IA32_APIC_BASE_MSR: u32 = 0x1B;
const APIC_BASE_ENABLE: u64 = 1 << 11;
const DEFAULT_LAPIC_PHYS: u64 = 0xFEE0_0000;
const DEFAULT_IOAPIC_PHYS: u64 = 0xFEC0_0000;

/// Resolves the physical Local APIC MMIO base from ACPI MADT (falling back to `0xFEE0_0000`).
fn madt_lapic_phys() -> u64 {
    let acpi = super::acpi_tables::info();
    if acpi.valid && acpi.local_apic_addr != 0 {
        acpi.local_apic_addr
    } else {
        DEFAULT_LAPIC_PHYS
    }
}

/// Configures and enables the Local APIC (xAPIC) via MSR and MADT-discovered MMIO.
pub unsafe fn init_local_apic() {
    let eax: u32;
    let edx: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx") IA32_APIC_BASE_MSR,
        out("eax") eax,
        out("edx") edx,
        options(nomem, nostack, preserves_flags)
    );
    let mut apic_base_msr = ((edx as u64) << 32) | (eax as u64);

    if (apic_base_msr & APIC_BASE_ENABLE) == 0 {
        apic_base_msr |= APIC_BASE_ENABLE;
        core::arch::asm!(
            "wrmsr",
            in("ecx") IA32_APIC_BASE_MSR,
            in("eax") (apic_base_msr as u32),
            in("edx") ((apic_base_msr >> 32) as u32),
            options(nomem, nostack, preserves_flags)
        );
    }

    let msr_phys = apic_base_msr & 0x000F_FFFF_FFFF_F000;
    let lapic_phys = if msr_phys != 0 { msr_phys } else { madt_lapic_phys() };
    let lapic_virt = crate::mm::map_kernel_mmio(lapic_phys, 4096);
    LAPIC_VIRT.store(lapic_virt, Ordering::Release);

    // SVR (Spurious Interrupt Vector Register, offset 0x0F0):
    // Bit 8 = 1 (APIC Software Enable), Vector = 0xFF (255) -> 0x1FF
    let svr_ptr = (lapic_virt + 0x0F0) as *mut u32;
    core::ptr::write_volatile(svr_ptr, 0x1FF);

    // TPR (Task Priority Register, offset 0x080): 0 to accept all interrupts
    let tpr_ptr = (lapic_virt + 0x080) as *mut u32;
    core::ptr::write_volatile(tpr_ptr, 0);

    // EOI (End of Interrupt, offset 0x0B0): clear pending
    let eoi_ptr = (lapic_virt + 0x0B0) as *mut u32;
    core::ptr::write_volatile(eoi_ptr, 0);
}

/// Probes the MADT-discovered I/O APIC MMIO registers (`IOAPICID` and `IOAPICVER`).
unsafe fn probe_madt_ioapic() -> (u64, u32, u32) {
    let acpi = super::acpi_tables::info();
    let ioapic_phys = if acpi.valid && acpi.ioapic_count > 0 && acpi.ioapics[0].mmio_addr != 0 {
        acpi.ioapics[0].mmio_addr as u64
    } else {
        DEFAULT_IOAPIC_PHYS
    };
    let virt = crate::mm::map_kernel_mmio(ioapic_phys, 4096);
    let ioregsel = virt as *mut u32;
    let iowin = (virt + 0x10) as *const u32;
    core::ptr::write_volatile(ioregsel, 0x00);
    let id_reg = core::ptr::read_volatile(iowin);
    core::ptr::write_volatile(ioregsel, 0x01);
    let ver_reg = core::ptr::read_volatile(iowin);
    (ioapic_phys, id_reg, ver_reg)
}

/// Trampoline entry point invoked directly by Limine on each Application Processor (`AP`).
#[no_mangle]
pub unsafe extern "C" fn ap_entry(cpu: &limine::mp::Cpu) -> ! {
    let cpu_id = cpu.id as usize;
    let lapic_id = cpu.lapic_id;
    let stack_top = (&raw const AP_STACKS[cpu_id.min(15)])
        .cast::<u8>()
        .add(AP_STACK_SIZE) as u64;

    // Switch stack to our dedicated 16 KiB AP stack and jump to ap_startup_continue
    core::arch::asm!(
        "mov rsp, {stack}",
        "push 0",
        "call {entry}",
        stack = in(reg) stack_top,
        entry = sym ap_startup_continue,
        in("rdi") cpu_id,
        in("rsi") lapic_id as u64,
        options(noreturn)
    );
}

/// Second stage AP initialization executing on the dedicated 16 KiB `AP_STACK`.
unsafe extern "C" fn ap_startup_continue(cpu_id: usize, lapic_id: u64) -> ! {
    // 1. Carga la tabla de paginas del kernel (MASTER_KERNEL_PML4)
    let master_cr3 = crate::mm::master_kernel_cr3();
    crate::mm::switch_cr3(master_cr3);

    // 2. Inicializa la GDT y TSS dedicados para este nucleo
    let stack_top = (&raw const AP_STACKS[cpu_id.min(15)])
        .cast::<u8>()
        .add(AP_STACK_SIZE) as u64;
    super::gdt::init_ap_gdt(cpu_id, stack_top);

    // 3. Inicializa la IDT compartida en este AP
    super::idt::load_idtr();

    // 4. Activa las caracteristicas de hardware de punto flotante y SIMD (FPU, SSE/SSE2, XSAVE/AVX)
    super::fpu::init_core();

    // 5. Configura PerCpu GS.base y MSRs de syscall (STAR/LSTAR/FMASK/EFER) en este AP
    super::syscall::init_core(cpu_id);

    // 6. Configura el Local APIC del nucleo y su Timer periódico para AP (vector 0x40)
    init_local_apic();
    init_ap_lapic_timer();

    // 7. Registra el AP como activo en el planificador
    crate::scheduler::register_ap_online(cpu_id as u32, lapic_id as u32);

    crate::early_println!(
        "[ostd::smp] AP Core #{} (LAPIC ID {}) despierto y online en Anillo 0 | GDT/TSS/IDT OK | LAPIC activo",
        cpu_id,
        lapic_id
    );

    // 7. Pasa al bucle de ejecucion concurrente del planificador
    crate::scheduler::ap_worker_loop(cpu_id as u32);
}

/// Initializes SMP on the BSP, enumerates APs via ACPI MADT APIC IDs, and awakens them.
pub fn init() {
    let (ioapic_phys, ioapic_id, ioapic_ver) = unsafe {
        init_local_apic();
        probe_madt_ioapic()
    };
    let lapic_phys = madt_lapic_phys();
    let max_redirs = ((ioapic_ver >> 16) & 0xFF) + 1;
    let acpi = super::acpi_tables::info();

    let Some(mp_response) = crate::boot::smp_response() else {
        crate::early_println!("[ostd::smp] Protocolo Limine SMP no devolvio respuesta. Modo Uniprocesador (UP).");
        return;
    };

    let bsp_lapic_id = mp_response.bsp_lapic_id();
    crate::scheduler::register_bsp(bsp_lapic_id);

    let cpus = mp_response.cpus();
    crate::early_println!(
        "[ostd::smp] MADT LAPIC={:#x} IOAPIC={:#x} (ID={}, Ver={:#x}, Redirs={}) | BSP LAPIC ID={} | Limine CPUs={}",
        lapic_phys,
        ioapic_phys,
        (ioapic_id >> 24) & 0xF,
        ioapic_ver & 0xFF,
        max_redirs,
        bsp_lapic_id,
        cpus.len()
    );

    let mut woken = [false; 16];
    if acpi.valid && acpi.cpu_count > 0 {
        let count = (acpi.cpu_count as usize).min(acpi.local_apic_ids.len());
        crate::early_println!(
            "[ostd::smp] Encendido SMP guiado por MADT APIC IDs: {:?}",
            &acpi.local_apic_ids[..count]
        );
        for &madt_apic_id in &acpi.local_apic_ids[..count] {
            if madt_apic_id as u32 == bsp_lapic_id {
                continue;
            }
            for (idx, cpu) in cpus.iter().enumerate() {
                if idx < woken.len() && !woken[idx] && cpu.lapic_id as u8 == madt_apic_id {
                    woken[idx] = true;
                    crate::early_println!(
                        "[ostd::smp] Despertando AP Core #{} via MADT (LAPIC ID {})...",
                        cpu.id,
                        cpu.lapic_id
                    );
                    cpu.goto_address.write(ap_entry);
                    break;
                }
            }
        }
    }

    for (idx, cpu) in cpus.iter().enumerate() {
        if cpu.lapic_id != bsp_lapic_id && idx < woken.len() && !woken[idx] {
            woken[idx] = true;
            crate::early_println!(
                "[ostd::smp] Despertando AP Core #{} (LAPIC ID {})...",
                cpu.id,
                cpu.lapic_id
            );
            cpu.goto_address.write(ap_entry);
        }
    }

    // Esperar sincronizacion de los APs
    let mut spins = 0usize;
    while crate::scheduler::online_cpu_count() < cpus.len() && spins < 2_000_000 {
        core::hint::spin_loop();
        spins += 1;
    }

    let active_cores = crate::scheduler::online_cpu_count();
    crate::early_println!(
        "[ostd::smp] Inicializacion SMP completada: {} nucleos activos.",
        active_cores
    );
    crate::fb_println!(
        "[ostd::smp] Limine SMP: {} nucleos activos (BSP ID={}, APs={})",
        active_cores,
        bsp_lapic_id,
        active_cores.saturating_sub(1)
    );
}

/// Returns the hardware LAPIC ID of the calling CPU from the xAPIC MMIO ID register.
#[inline]
pub fn current_lapic_id() -> u32 {
    let base = LAPIC_VIRT.load(Ordering::Acquire);
    if base == 0 {
        return 0;
    }
    // SAFETY: Reading mapped xAPIC ID register (offset 0x020) in Ring 0.
    unsafe { core::ptr::read_volatile((base + 0x020) as *const u32) >> 24 }
}

/// Signals End-Of-Interrupt (EOI) to the calling core's Local APIC.
#[inline]
pub fn acknowledge_lapic_eoi() {
    let base = LAPIC_VIRT.load(Ordering::Acquire);
    if base != 0 {
        // SAFETY: Writing 0 to mapped xAPIC EOI register (offset 0x0B0) in Ring 0.
        unsafe { core::ptr::write_volatile((base + 0x0B0) as *mut u32, 0) };
    }
}

/// Enables the AP Local APIC Timer in periodic mode on IDT vector `0x40`.
unsafe fn init_ap_lapic_timer() {
    let base = LAPIC_VIRT.load(Ordering::Acquire);
    if base == 0 {
        return;
    }
    // SAFETY: Configuring xAPIC Timer Divide (0x3E0), LVT Timer (0x320), and Initial Count (0x380).
    unsafe {
        core::ptr::write_volatile((base + 0x3E0) as *mut u32, 0x3);
        core::ptr::write_volatile((base + 0x320) as *mut u32, 0x0002_0040);
        core::ptr::write_volatile((base + 0x380) as *mut u32, 0x0008_0000);
    }
}

core::arch::global_asm!(
    ".global __isr_ap_timer_irq",
    "__isr_ap_timer_irq:",
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
    "    jz .Lap_timer_kentry",
    "    swapgs",
    ".Lap_timer_kentry:",
    "    sub rsp, 512",
    "    and rsp, -16",
    "    fxsave64 [rsp]",
    "    call __rust_ap_timer_irq_handler",
    "    fxrstor64 [rsp]",
    "    mov rsp, rbp",
    "    test byte ptr [rbp + 88], 3",
    "    jz .Lap_timer_kexit",
    "    swapgs",
    ".Lap_timer_kexit:",
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
);

#[no_mangle]
extern "C" fn __rust_ap_timer_irq_handler() {
    crate::scheduler::on_ap_timer_irq(current_lapic_id());
    acknowledge_lapic_eoi();
}
