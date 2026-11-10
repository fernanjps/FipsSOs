#![forbid(unsafe_code)]

//! FipSOs Process Launch & Userland Orchestrator (`kernel_main::process_launch`).
//!
//! Responsible for loading and spawning initial Ring 3 processes:
//! POSIX Shell (`/bin/fipsos_sh.elf`), Graphical Desktop (`/bin/desktop_ui.elf`),
//! SIMD Background Worker, NPU Inference (`/bin/npu_infer.elf`), and C++20 Runtime.

use alloc::sync::Arc;
use alloc::vec::Vec;
use kernel_objects::{
    Channel, KernelObject, Process, ProcessState, Rights, SchedulerRegistry, Thread, Vmo,
};
use linux_abi::{Elf64Loader, FD_FB0};

/// Safe machine code builder for generating the contingency fallback ELF64 binary.
struct Ring3CodeBuilder {
    bytes: Vec<u8>,
}

impl Ring3CodeBuilder {
    fn new() -> Self {
        Self {
            bytes: Vec::with_capacity(256),
        }
    }

    fn emit(&mut self, slice: &[u8]) {
        self.bytes.extend_from_slice(slice);
    }

    fn emit_sys_write(&mut self, buf_vaddr: u32, len: u32) {
        self.emit(&[0xB8, 0x01, 0x00, 0x00, 0x00]); // mov eax, 1 (SYS_WRITE)
        self.emit(&[0xBF, 0x01, 0x00, 0x00, 0x00]); // mov edi, 1 (FD_STDOUT)
        self.emit(&[0xBE]);
        self.emit(&buf_vaddr.to_le_bytes()); // mov esi, buf_vaddr
        self.emit(&[0xBA]);
        self.emit(&len.to_le_bytes()); // mov edx, len
        self.emit(&[0x0F, 0x05]); // syscall
    }

    fn finish(self) -> Vec<u8> {
        self.bytes
    }
}

/// Builds an emergency fallback ELF64 executable if `/bin/fipsos_sh.elf` is not found on disk.
pub fn build_compact_fallback_shell_elf64() -> Vec<u8> {
    const CODE_BASE: u32 = ostd::arch::syscall::USER_CODE_BASE_VADDR as u32;
    const RODATA_OFFSET: usize = 0x1000;
    const RODATA_BASE: u32 = CODE_BASE + (RODATA_OFFSET as u32);

    let banner = b"[FipSOs::fallback_shell] Modo emergencia: binario principal no disponible. Saliendo...\n";
    let mut rodata = [0u8; 256];
    rodata[..banner.len()].copy_from_slice(banner);

    let mut asm = Ring3CodeBuilder::new();
    asm.emit_sys_write(RODATA_BASE, banner.len() as u32);
    asm.emit(&[0xB8, 0x3C, 0x00, 0x00, 0x00]); // mov eax, 60 (SYS_EXIT)
    asm.emit(&[0x31, 0xFF]); // xor edi, edi (status = 0)
    asm.emit(&[0x0F, 0x05]); // syscall

    let machine_code = asm.finish();
    Elf64Loader::build_single_segment_elf64(
        ostd::arch::syscall::USER_CODE_BASE_VADDR,
        &machine_code,
        RODATA_OFFSET,
        &rodata[..banner.len()],
    )
}

/// Spawns the initial userland environment and executes the Ring 3 transition.
pub fn launch_system_processes(vmo: Arc<Vmo>, endpoint_b: Arc<Channel>) {
    // 1. Gestor de Sesion: Determinar si se solicito arranque grafico directo ('g' o flag "gui")
    let cmdline_gui = ostd::boot::kernel_cmdline()
        .map(|cmd| cmd.contains("gui") || cmd.contains("desktop"))
        .unwrap_or(false);
    let key_gui = ostd::arch::keyboard::try_pop_key_nonblocking()
        .map(|k| k == b'g' || k == b'G')
        .unwrap_or(false);
    let boot_gui = cmdline_gui || key_gui;

    let target_bin = if boot_gui {
        dual_println!("[FipSOs::session] Arranque grafico solicitado ('g' o cmdline). Iniciando Desktop UI...");
        "/bin/desktop_ui.elf"
    } else {
        "/bin/fipsos_sh.elf"
    };

    let (elf_image, is_compiled_elf) = match linux_abi::read_all_by_path(target_bin) {
        Ok(bytes) => {
            dual_println!("[FipSOs::boot] Cargando binario inicial \"{}\" ({} bytes)...", target_bin, bytes.len());
            (bytes, true)
        }
        Err(_) => {
            dual_println!("[FipSOs::boot] Binario \"{}\" no disponible, usando fallback en codigo maquina.", target_bin);
            (build_compact_fallback_shell_elf64(), false)
        }
    };

    let shell_process = Process::new(
        if is_compiled_elf { target_bin } else { "fipsos-shell.elf" },
        ostd::arch::syscall::USER_CODE_BASE_VADDR,
    );
    ostd::mm::switch_cr3(shell_process.cr3_phys());
    let loaded_elf = Elf64Loader::load(&elf_image, 0)
        .expect("Fallo al cargar el binario ELF64 inicial en el CR3 aislado");

    let initial_rsp = if is_compiled_elf {
        ostd::arch::syscall::setup_sysv_abi_user_stack(
            target_bin,
            loaded_elf.entry_point,
            loaded_elf.phdr_vaddr,
            loaded_elf.phentsize,
            loaded_elf.phnum,
        ).expect("Fallo al configurar stack System V ABI para proceso inicial")
    } else {
        0x0080_3FF0
    };

    let proc_vmo_handle = shell_process.insert_handle(
        Arc::clone(&vmo) as Arc<dyn KernelObject>,
        Rights::READ | Rights::WRITE | Rights::MAP,
    );
    let proc_chan_handle = shell_process.insert_handle(
        Arc::clone(&endpoint_b) as Arc<dyn KernelObject>,
        Rights::READ | Rights::WRITE | Rights::TRANSFER,
    );
    let shell_thread = Thread::new(
        "shell-main",
        shell_process.koid(),
        loaded_elf.entry_point,
        initial_rsp,
    );

    let (daemon_entry, daemon_stack_top) = ostd::scheduler::daemon_entry_and_stack_top();
    let daemon_process = Process::new_kernel("simd-worker", daemon_entry);
    let daemon_thread = Thread::new(
        "simd-worker",
        daemon_process.koid(),
        daemon_entry,
        daemon_stack_top,
    );

    let infer_process = Process::new("/bin/npu_infer.elf", 0x0040_0000);
    let infer_thread = Thread::new(
        "rust-elf64",
        infer_process.koid(),
        0x0040_0000,
        0x0080_3FF8,
    );

    let avatar_cpp_process = Process::new("/bin/avatar_cpp.elf", 0x0040_0000);
    let avatar_cpp_thread = Thread::new(
        "cpp20-elf64",
        avatar_cpp_process.koid(),
        0x0040_0000,
        0x0080_3FF8,
    );

    let desktop_ui_process = Process::new("/bin/desktop_ui.elf", 0x0040_0000);
    let desktop_ui_thread = Thread::new(
        "gui-compositor",
        desktop_ui_process.koid(),
        0x0040_0000,
        0x0080_3FF8,
    );

    SchedulerRegistry::register_boot_entries(
        Arc::clone(&shell_process),
        Arc::clone(&shell_thread),
        Arc::clone(&daemon_process),
        Arc::clone(&daemon_thread),
        Arc::clone(&infer_process),
        Arc::clone(&infer_thread),
        Arc::clone(&avatar_cpp_process),
        Arc::clone(&avatar_cpp_thread),
        Arc::clone(&desktop_ui_process),
        Arc::clone(&desktop_ui_thread),
    );

    dual_println!(
        "[FipSOs::cr3] PML4 Aislados OK -> Kernel={:#x} | Shell={:#x} | DesktopUI={:#x} | Rust={:#x} | C++20={:#x}",
        daemon_process.cr3_phys(),
        shell_process.cr3_phys(),
        desktop_ui_process.cr3_phys(),
        infer_process.cr3_phys(),
        avatar_cpp_process.cr3_phys()
    );
    dual_println!(
        "[FipSOs::process] Proceso \"{}\" ({}) Thread \"{}\" ({}) CR3={:#x} Estado={} | Handles=[{}, {}] | Entry={:#x}",
        shell_process.name(),
        shell_process.koid(),
        shell_thread.name(),
        shell_thread.koid(),
        shell_process.cr3_phys(),
        shell_process.state(),
        proc_vmo_handle,
        proc_chan_handle,
        shell_process.entry_point()
    );
    dual_println!(
        "[Thread] Planificador ASM switch.s activo: Shell({}) + Daemon({} @ 16KiB) + DesktopUI({}) + Rust({}) + C++20({})",
        shell_thread.koid(),
        daemon_thread.koid(),
        desktop_ui_thread.koid(),
        infer_thread.koid(),
        avatar_cpp_thread.koid()
    );

    let exit_code = if is_compiled_elf {
        ostd::arch::syscall::enter_ring3_sysv(loaded_elf.entry_point, initial_rsp)
            .expect("Fallo en la transicion a Anillo 3 con System V ABI")
    } else {
        ostd::arch::syscall::enter_ring3_at(shell_process.entry_point())
            .expect("Fallo en la transicion a Anillo 3")
    };
    shell_process.set_state(ProcessState::Exited(exit_code));

    // Si se arranco inicialmente en modo GUI y el usuario salio con 'q' o Salir,
    // transicionar fluidamente a la consola TTY de /bin/fipsos_sh.elf!
    if boot_gui {
        dual_println!("[FipSOs::session] Salida de DesktopUI detectada. Conmutando a Shell TTY /bin/fipsos_sh.elf...");
        if let Ok(sh_bytes) = linux_abi::read_all_by_path("/bin/fipsos_sh.elf") {
            let sh_proc = Process::new("/bin/fipsos_sh.elf", ostd::arch::syscall::USER_CODE_BASE_VADDR);
            ostd::mm::switch_cr3(sh_proc.cr3_phys());
            if let Ok(sh_loaded) = Elf64Loader::load(&sh_bytes, 0) {
                if let Ok(sh_rsp) = ostd::arch::syscall::setup_sysv_abi_user_stack(
                    "/bin/fipsos_sh.elf",
                    sh_loaded.entry_point,
                    sh_loaded.phdr_vaddr,
                    sh_loaded.phentsize,
                    sh_loaded.phnum,
                ) {
                    let _ = ostd::arch::syscall::enter_ring3_sysv(sh_loaded.entry_point, sh_rsp);
                }
            }
        }
    }

    // Verificar desde el Kernel los pixeles Zero-Copy dibujados por Anillo 3 en el VMO de /dev/fb0
    let mut fb_sample = [0u8; 4];
    linux_abi::read_device_vmo(FD_FB0, 0, &mut fb_sample)
        .expect("Fallo al leer VMO de /dev/fb0 desde el kernel");
    let first_pixel = u32::from_le_bytes(fb_sample);

    dual_println!(
        "[FipSOs::zerocopy] Verificacion Kernel en VMO /dev/fb0: Pixel[0]={:#010x} | Proceso {} Estado={}.",
        first_pixel,
        shell_process.koid(),
        shell_process.state()
    );
}
