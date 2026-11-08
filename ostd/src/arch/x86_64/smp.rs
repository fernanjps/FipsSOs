//! Symmetric Multi-Processing (SMP) & AP Bootstrapping (`ostd::arch::x86_64::smp`).
//!
//! Conforms to the Limine MP / SMP Boot Protocol:
//! - Enumerates all Application Processors (APs) reported by ACPI / Firmware.
//! - Awakes each AP via `goto_address`.
//! - Configures per-core page tables (`MASTER_KERNEL_PML4`), dedicated 16 KiB AP stack,
//!   per-core GDT/TSS, IDT, FPU/SSE2, and Local APIC (xAPIC).
//! - Enters the concurrent task execution loop in `ostd::scheduler`.


/// Dedicated 16 KiB stack per Application Processor (AP).
pub const AP_STACK_SIZE: usize = 16 * 1024;

#[repr(C, align(16))]
struct ApStack([u8; AP_STACK_SIZE]);

static mut AP_STACKS: [ApStack; 4] = [
    ApStack([0; AP_STACK_SIZE]),
    ApStack([0; AP_STACK_SIZE]),
    ApStack([0; AP_STACK_SIZE]),
    ApStack([0; AP_STACK_SIZE]),
];

const IA32_APIC_BASE_MSR: u32 = 0x1B;
const APIC_BASE_ENABLE: u64 = 1 << 11;

/// Configures and enables the Local APIC (xAPIC) via MSR and MMIO.
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

    let lapic_phys = apic_base_msr & 0x000F_FFFF_FFFF_F000;
    let lapic_virt = crate::mm::map_kernel_mmio(lapic_phys, 4096);

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

/// Trampoline entry point invoked directly by Limine on each Application Processor (`AP`).
#[no_mangle]
pub unsafe extern "C" fn ap_entry(cpu: &limine::mp::Cpu) -> ! {
    let cpu_id = cpu.id as usize;
    let lapic_id = cpu.lapic_id;
    let stack_top = (&raw const AP_STACKS[cpu_id.min(3)])
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
    let stack_top = (&raw const AP_STACKS[cpu_id.min(3)])
        .cast::<u8>()
        .add(AP_STACK_SIZE) as u64;
    super::gdt::init_ap_gdt(cpu_id, stack_top);

    // 3. Inicializa la IDT compartida en este AP
    super::idt::load_idtr();

    // 4. Activa las caracteristicas de hardware de punto flotante y SIMD (FPU, SSE/SSE2)
    super::fpu::init();

    // 5. Configura el Local APIC del nucleo
    init_local_apic();

    // 6. Registra el AP como activo en el planificador
    crate::scheduler::register_ap_online(cpu_id as u32, lapic_id as u32);

    crate::early_println!(
        "[ostd::smp] AP Core #{} (LAPIC ID {}) despierto y online en Anillo 0 | GDT/TSS/IDT OK | LAPIC activo",
        cpu_id,
        lapic_id
    );

    // 7. Pasa al bucle de ejecucion concurrente del planificador
    crate::scheduler::ap_worker_loop(cpu_id as u32);
}

/// Initializes SMP on the BSP, enumerates APs from Limine, and awakens them.
pub fn init() {
    unsafe {
        init_local_apic();
    }

    let Some(mp_response) = crate::boot::smp_response() else {
        crate::early_println!("[ostd::smp] Protocolo Limine SMP no devolvio respuesta. Modo Uniprocesador (UP).");
        return;
    };

    let bsp_lapic_id = mp_response.bsp_lapic_id();
    crate::scheduler::register_bsp(bsp_lapic_id);

    let cpus = mp_response.cpus();
    crate::early_println!(
        "[ostd::smp] Limine reporto {} procesador(es). BSP LAPIC ID={}",
        cpus.len(),
        bsp_lapic_id
    );

    for cpu in cpus {
        if cpu.lapic_id != bsp_lapic_id {
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
