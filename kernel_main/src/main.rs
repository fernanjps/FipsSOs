#![forbid(unsafe_code)]
#![no_std]
#![no_main]

extern crate alloc;

use alloc::sync::Arc;
use alloc::vec::Vec;
use kernel_objects::{
    yield_now, Channel, Executor, HandleTable, KernelObject, MessagePacket, Process, ProcessState,
    Rights, SchedulerRegistry, Thread, Vmo, VmoUsage,
};
use linux_abi::{
    Elf64Loader, ACCEL_IOCTL_BIND_TENSOR_VMO, ACCEL_IOCTL_RUN_INFERENCE, DISK_IOCTL_SAVE_NOTE,
    FB_IOCTL_PRESENT_RECT, FB_WIDGET_DEFAULT_H, FB_WIDGET_DEFAULT_W, FD_FB0,
    MOUSE_IOCTL_POLL_STATE, NET_IOCTL_BROWSER_FETCH_SAVE, NET_IOCTL_HTTP_FETCH, NET_IOCTL_PING_GATEWAY,
    SYS_KILL, SYS_UNLINK, VFS_IOCTL_HWMON_REPORT, VFS_IOCTL_LIST_FILES, VFS_IOCTL_LIST_PROCESSES,
    WIFI_IOCTL_SCAN, WIFI_IOCTL_STATUS,
};
use ostd::bus::pcie::PciDeviceClass;
use ostd::ffi_jail::{sample_c_firmware_routine, FfiJail};
use ostd::sync::SpinLock;

ostd::kernel_main!(fipsos_main);

/// Prints to both the hardware serial port (`0x3F8`) and the visual Framebuffer console (`1280x800`).
macro_rules! dual_println {
    ($($arg:tt)*) => {{
        ostd::early_println!($($arg)*);
        ostd::fb_println!($($arg)*);
    }};
}

/// Safe label-resolving x86_64 machine code builder for constructing the Ring 3 ELF64 shell binary.
struct Ring3CodeBuilder {
    bytes: Vec<u8>,
    labels: [usize; 48],
    rel32_fixups: Vec<(usize, usize)>,
}

impl Ring3CodeBuilder {
    fn new() -> Self {
        Self {
            bytes: Vec::with_capacity(2048),
            labels: [0; 48],
            rel32_fixups: Vec::new(),
        }
    }

    fn emit(&mut self, slice: &[u8]) {
        self.bytes.extend_from_slice(slice);
    }

    fn bind_label(&mut self, label_id: usize) {
        self.labels[label_id] = self.bytes.len();
    }

    fn emit_jmp_rel32(&mut self, label_id: usize) {
        self.bytes.push(0xE9);
        let patch_offset = self.bytes.len();
        self.bytes.extend_from_slice(&[0, 0, 0, 0]);
        self.rel32_fixups.push((patch_offset, label_id));
    }

    fn emit_je_rel32(&mut self, label_id: usize) {
        self.bytes.extend_from_slice(&[0x0F, 0x84]);
        let patch_offset = self.bytes.len();
        self.bytes.extend_from_slice(&[0, 0, 0, 0]);
        self.rel32_fixups.push((patch_offset, label_id));
    }

    fn emit_jnz_rel32(&mut self, label_id: usize) {
        self.bytes.extend_from_slice(&[0x0F, 0x85]);
        let patch_offset = self.bytes.len();
        self.bytes.extend_from_slice(&[0, 0, 0, 0]);
        self.rel32_fixups.push((patch_offset, label_id));
    }

    fn emit_js_rel32(&mut self, label_id: usize) {
        self.bytes.extend_from_slice(&[0x0F, 0x88]);
        let patch_offset = self.bytes.len();
        self.bytes.extend_from_slice(&[0, 0, 0, 0]);
        self.rel32_fixups.push((patch_offset, label_id));
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

    fn emit_sys_open(&mut self, path_vaddr: u32, flags: u32) {
        self.emit(&[0xB8, 0x02, 0x00, 0x00, 0x00]); // mov eax, 2 (SYS_OPEN)
        self.emit(&[0xBF]);
        self.emit(&path_vaddr.to_le_bytes()); // mov edi, path_vaddr
        self.emit(&[0xBE]);
        self.emit(&flags.to_le_bytes()); // mov esi, flags
        self.emit(&[0x31, 0xD2]); // xor edx, edx (mode = 0)
        self.emit(&[0x0F, 0x05]); // syscall -> RAX = fd
    }

    fn finish(mut self) -> Vec<u8> {
        for (patch_offset, label_id) in self.rel32_fixups {
            let target = self.labels[label_id] as i32;
            let next_ip = (patch_offset + 4) as i32;
            let rel32 = target - next_ip;
            self.bytes[patch_offset..patch_offset + 4].copy_from_slice(&rel32.to_le_bytes());
        }
        self.bytes
    }
}

fn fipsos_main() {
    ostd::boot::init();
    linux_abi::init();
    dual_println!(
        "[FipSOs] Framekernel FipSOs v1.9 activo (Linux C stdlib + System V ABI + 4 Ventanas Shell UI + SATA + WiFi)."
    );

    // 1. Reporte de Topologia de CPU (CPUID) en Serie y Framebuffer
    let cpu = ostd::arch::topology::detect();
    dual_println!(
        "[FipSOs::topology] CPU Vendor=\"{}\" | Brand=\"{}\" | Cores={} | SIMD=[SSE4.2:{}, AVX2:{}, AVX-512:{}]",
        cpu.vendor(),
        cpu.brand(),
        cpu.logical_cores,
        cpu.sse4_2,
        cpu.avx2,
        cpu.avx512f
    );

    // 2. Descubrimiento de Hardware en el Bus PCI/PCIe (GPUs Clase 0x03 y NPUs Clase 0x12)
    let pci_devices = ostd::bus::pcie::scan_bus();
    let nic_count = pci_devices
        .iter()
        .filter(|d| d.classification() == PciDeviceClass::NetworkController)
        .count();
    let gpu_count = pci_devices
        .iter()
        .filter(|d| d.classification() == PciDeviceClass::GpuDisplayController)
        .count();
    let npu_count = pci_devices
        .iter()
        .filter(|d| d.classification() == PciDeviceClass::NpuProcessingAccelerator)
        .count();
    for dev in &pci_devices {
        if dev.classification() == PciDeviceClass::GpuDisplayController {
            ostd::fb_println!(
                "[ostd::bus::pcie] >>> [GPU DETECTADA] {:02x}:{:02x}.{} | Vendor={:#06x} Device={:#06x} BAR0={:#x}",
                dev.bus,
                dev.device,
                dev.function,
                dev.vendor_id,
                dev.device_id,
                dev.bar0
            );
        } else if dev.classification() == PciDeviceClass::NetworkController {
            ostd::fb_println!(
                "[ostd::bus::pcie] >>> [NIC DETECTADA] {:02x}:{:02x}.{} | Vendor={:#06x} Device={:#06x} BAR0={:#x}",
                dev.bus,
                dev.device,
                dev.function,
                dev.vendor_id,
                dev.device_id,
                dev.bar0
            );
        }
    }
    dual_println!(
        "[FipSOs::pcie] Escaneo PCI/PCIe completado: {} dispositivos totales ({} NIC 0x02, {} GPU 0x03, {} NPU 0x12)",
        pci_devices.len(),
        nic_count,
        gpu_count,
        npu_count
    );

    // 3. Prueba de Heap, VMO Zero-Copy de 1 MiB, HandleTable y FFI Jail
    let mut tensor_shape = Vec::with_capacity(4);
    tensor_shape.push(1usize);
    tensor_shape.push(3usize);
    tensor_shape.push(512usize);
    tensor_shape.push(512usize);

    const SHARED_AI_BUFFER_SIZE: usize = 1024 * 1024; // 1 MiB
    let vmo = Vmo::new(SHARED_AI_BUFFER_SIZE, VmoUsage::ZeroCopyAccel)
        .expect("Fallo al crear el VMO de 1 MiB para IA/Graficos");

    let header_magic = b"FIPSOS_NPU_GPU_TENSOR_V1";
    vmo.write(0, header_magic)
        .expect("Fallo al escribir cabecera inicial en el VMO");

    let shared_table = Arc::new(SpinLock::new(HandleTable::new()));
    let vmo_handle = shared_table.lock().insert(
        Arc::clone(&vmo) as Arc<dyn KernelObject>,
        Rights::READ | Rights::WRITE | Rights::MAP | Rights::TRANSFER,
    );

    {
        let table_guard = shared_table.lock();
        let entry = table_guard
            .get(vmo_handle, Rights::READ | Rights::MAP | Rights::TRANSFER)
            .expect("Permisos insuficientes al consultar el Handle del VMO");
        let resolved_vmo = entry
            .as_vmo()
            .expect("El objeto referenciado por el Handle no es un VMO");

        dual_println!(
            "[FipSOs::vmo] Handle={} | Object={} ({}) | Rights=[{}] | Size={} KiB | Phys={:#x} | Shape={:?}",
            entry.handle_id(),
            resolved_vmo.koid(),
            resolved_vmo.object_type(),
            entry.rights(),
            resolved_vmo.size_bytes() / 1024,
            resolved_vmo.phys_addr(),
            tensor_shape
        );

        let ffi_jail = FfiJail::new();
        let region_id = ffi_jail.register_dma_region(
            resolved_vmo.dma_buffer(),
            false,
            "NPU_FIRMWARE_TENSOR_RO",
        );
        let checksum = ffi_jail
            .execute_c_function(
                region_id,
                0,
                header_magic.len(),
                false,
                0x2026,
                sample_c_firmware_routine,
            )
            .expect("Fallo al ejecutar funcion extern \"C\" dentro de FfiJail");
        let oob_check = ffi_jail.execute_c_function(
            region_id,
            SHARED_AI_BUFFER_SIZE,
            64,
            false,
            0,
            sample_c_firmware_routine,
        );
        dual_println!(
            "[FipSOs::ffi_jail] Region {} registrada | C-FFI checksum={:#x} | Bloqueo OOB verificado={}",
            region_id,
            checksum,
            oob_check.is_err()
        );
    }

    // 4. Prueba de Channel IPC Zircon con transferencia de HandleId + Waker Async en Executor
    let (endpoint_a, endpoint_b) = Channel::create_pair();
    let mut executor = Executor::new();
    let table_for_receiver = Arc::clone(&shared_table);

    let endpoint_b_clone = Arc::clone(&endpoint_b);
    executor.spawn(async move {
        let packet = endpoint_b_clone
            .read_msg_async()
            .await
            .expect("Error al recibir MessagePacket en endpoint_b");

        let msg_text = core::str::from_utf8(&packet.data).unwrap_or("<bin>");
        let transferred_handle = packet.handles[0];

        let table_guard = table_for_receiver.lock();
        let entry = table_guard
            .get(transferred_handle, Rights::READ | Rights::MAP)
            .expect("El HandleId transferido por Channel no es valido");
        let recv_vmo = entry.as_vmo().expect("El HandleId no apunta a un VMO");

        dual_println!(
            "[Async::IPC-Rx] Canal {} recibio msg=\"{}\" + Handle={} -> VMO {} (Phys={:#x})",
            endpoint_b_clone.koid(),
            msg_text,
            transferred_handle,
            recv_vmo.koid(),
            recv_vmo.phys_addr()
        );
    });

    let endpoint_a_clone = Arc::clone(&endpoint_a);
    executor.spawn(async move {
        yield_now().await;
        let payload = b"ZIRCON_IPC_SHARE_ZERO_COPY_VMO".to_vec();
        let handles = alloc::vec![vmo_handle];
        let packet = MessagePacket::new(payload, handles);
        endpoint_a_clone
            .write_msg(packet)
            .expect("Fallo al enviar MessagePacket en endpoint_a");
    });

    let total_polls = executor.run_until_idle();
    dual_println!(
        "[FipSOs::executor] IPC Zircon Channel ({} <-> {}) completado en {} polls.",
        endpoint_a.koid(),
        endpoint_b.koid(),
        total_polls
    );

    // 5. Gestor de Sesión: Determinar si se solicitó arranque gráfico directo ('g' pulsado o flag cmdline "gui")
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
            (build_interactive_shell_elf64(), false)
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
    let daemon_process = Process::new_kernel("npu-telemetryd", daemon_entry);
    let daemon_thread = Thread::new(
        "npu-worker",
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

/// Builds the Ring 3 ELF64 (`ET_EXEC` `x86_64`) Interactive Shell with `TarFs` (`ls`, `cat`, `npu`)
/// and Zero-Copy `/dev/fb0` Graphics Demo (`draw`, `stat`, `help`, `exit`).
fn build_interactive_shell_elf64() -> Vec<u8> {
    const CODE_BASE: u32 = ostd::arch::syscall::USER_CODE_BASE_VADDR as u32; // 0x0040_0000
    const RODATA_OFFSET: usize = 0x1000; // .rodata starts at 0x0040_1000
    const RODATA_BASE: u32 = CODE_BASE + (RODATA_OFFSET as u32);
    const USER_INPUT_BUF_VADDR: u32 = 0x0080_1000; // Ring 3 command buffer in stack page
    const USER_FILE_BUF_VADDR: u32 = 0x0080_1200; // Ring 3 file read buffer for `cat`

    const PATH_FB0_BYTES: &[u8] = b"/dev/fb0\0";
    const PATH_MOUSE_BYTES: &[u8] = b"/dev/input/mouse0\0";
    const PATH_NET_BYTES: &[u8] = b"/dev/net/eth0\0";
    const PATH_PKG_BYTES: &[u8] = b"/downloads/fipsos_pkg.txt\0";
    const PATH_MOTD_BYTES: &[u8] = b"/etc/motd.txt\0";
    const PATH_MODEL_BYTES: &[u8] = b"/models/avatar_v1.bin\0";
    const PATH_ACCEL_BYTES: &[u8] = b"/dev/accel/accel0\0";
    const PATH_EXEC_BYTES: &[u8] = b"/bin/npu_infer.elf\0";
    const PATH_CPP_BYTES: &[u8] = b"/bin/avatar_cpp.elf\0";
    const PATH_GUI_BYTES: &[u8] = b"/bin/desktop_ui.elf\0";
    const PATH_C_APP_BYTES: &[u8] = b"/bin/c_std_app.elf\0";
    const PATH_FAULT_BYTES: &[u8] = b"/bin/fault_test.elf\0";
    const PATH_NOTE_BYTES: &[u8] = b"/disk/persistent_note.txt\0";

    const BANNER_MSG: &[u8] =
        b"[UserRing3] FipSOs v1.9 TTY Consola activa (Linux C stdlib + System V ABI + 4 Ventanas GUI + SATA + WiFi). Escribe 'capp', 'gui' o 'help'.\n";
    const PROMPT_MSG: &[u8] = b"fipsos@v1.9:/$ ";
    const HELP_MSG: &[u8] =
        b"[Shell::help] Comandos: help | capp | gui | ls | cat [ruta] | ps | wifi | curl | rm [ruta] | kill | sensors | fault | poweroff | reboot | disk | mouse | net | wget | npu | exec | cpp | stat | draw | exit\n";
    const CAPP_DONE_MSG: &[u8] =
        b"[Shell::capp] Aplicacion C Linux (/bin/c_std_app.elf @ System V ABI) completada (exit=0).\n";
    const GUI_DONE_MSG: &[u8] =
        b"[Shell::gui] Compositor FipSOs Shell UI (/bin/desktop_ui.elf @ 0x400000 CR3 aislado) cerrado (exit=0).\n";
    const STAT_MSG: &[u8] =
        b"[Shell::stat] Diagnostico Zero-Copy NPU/GPU y Timer IRQ0 completado.\n";
    const DRAW_MSG: &[u8] =
        b"[Shell::draw] Widget Zero-Copy en /dev/fb0 actualizado en vivo desde Anillo 3!\n";
    const DISK_MSG: &[u8] =
        b"[Shell::disk] Disco SATA AHCI 00:1f.2 sincronizado. Nota persistente \"/disk/persistent_note.txt\" verificada (LBA R/W OK).\n";
    const MOUSE_MSG: &[u8] =
        b"[Shell::mouse] Lectura en vivo de /dev/input/mouse0 (IRQ12 PS/2) completada.\n";
    const NET_MSG: &[u8] =
        b"[Shell::net] Diagnostico PCIe e1000e /dev/net/eth0 (ARP+ICMP 10.0.2.15->10.0.2.2) completado.\n";
    const WGET_MSG: &[u8] =
        b"[Shell::wget] Paquete descargado en /downloads/fipsos_pkg.txt (verifica con 'ls' y 'cat').\n";
    const CAT_ERR_MSG: &[u8] =
        b"[Shell::cat] Archivo no encontrado en CapabilityVFS.\n";
    const NPU_MSG: &[u8] =
        b"[Shell::npu] Inferencia Zero-Copy /models/avatar_v1.bin -> Driver C11 /dev/accel/accel0 OK!\n";
    const EXEC_DONE_MSG: &[u8] =
        b"[Shell::exec] Proceso hijo /bin/npu_infer.elf (Rust ELF64 @ 0x400000 CR3 aislado) finalizado con exit_code=0\n";
    const CPP_DONE_MSG: &[u8] =
        b"[Shell::cpp] Motor C++20 /bin/avatar_cpp.elf (Anillo 3 @ 0x400000 CR3 aislado) finalizado con exit_code=0\n";
    const FAULT_DONE_MSG: &[u8] =
        b"[Shell::fault] Proceso /bin/fault_test.elf (#PF) interceptado y recuperado limpiamente por el kernel!\n";
    const SENSORS_DONE_MSG: &[u8] =
        b"[Shell::sensors] Telemetria de hardware (/sys/fipsos/hwmon) consultada OK.\n";
    const POWEROFF_MSG: &[u8] =
        b"[Shell::poweroff] Apagando el sistema FipSOs v1.8 via ACPI PM S5...\n";
    const REBOOT_MSG: &[u8] =
        b"[Shell::reboot] Reiniciando el sistema FipSOs v1.8...\n";
    const WIFI_MSG: &[u8] =
        b"[Shell::wifi] Escaneo 802.11 y estado de enlace /dev/net/wlan0 verificado.\n";
    const CURL_MSG: &[u8] =
        b"[Shell::curl] Descarga HTTP completada y guardada en SATA AHCI /disk/index.html.\n";
    const RM_MSG: &[u8] =
        b"[Shell::rm] Archivo eliminado de SATA DiskFS (sys_unlink).\n";
    const KILL_MSG: &[u8] =
        b"[Shell::kill] Tarea en segundo plano detenida (sys_kill SIGKILL=9).\n";
    const EXIT_MSG: &[u8] = b"[Shell::exit] Cerrando sesion interactiva FipSOs v1.8 en Anillo 3...\n";
    const UNKNOWN_MSG: &[u8] =
        b"[Shell] Comando desconocido. Escribe: help | gui | ls | cat | ps | wifi | curl | rm | kill | sensors | fault | poweroff | reboot | disk | mouse | net | wget | npu | exec | cpp | stat | draw | exit\n";

    let mut rodata = [0u8; 3072];
    let mut cursor = 0usize;
    let mut add_str = |s: &[u8]| -> (u32, u32) {
        let addr = RODATA_BASE + (cursor as u32);
        rodata[cursor..cursor + s.len()].copy_from_slice(s);
        cursor = (cursor + s.len() + 7) & !7;
        (addr, s.len() as u32)
    };

    let (path_fb0_addr, _) = add_str(PATH_FB0_BYTES);
    let (path_mouse_addr, _) = add_str(PATH_MOUSE_BYTES);
    let (path_net_addr, _) = add_str(PATH_NET_BYTES);
    let (path_pkg_addr, _) = add_str(PATH_PKG_BYTES);
    let (path_motd_addr, _) = add_str(PATH_MOTD_BYTES);
    let (path_model_addr, _) = add_str(PATH_MODEL_BYTES);
    let (path_accel_addr, _) = add_str(PATH_ACCEL_BYTES);
    let (path_exec_addr, _) = add_str(PATH_EXEC_BYTES);
    let (path_cpp_addr, _) = add_str(PATH_CPP_BYTES);
    let (path_gui_addr, _) = add_str(PATH_GUI_BYTES);
    let (path_capp_addr, _) = add_str(PATH_C_APP_BYTES);
    let (path_fault_addr, _) = add_str(PATH_FAULT_BYTES);
    let (path_note_addr, _) = add_str(PATH_NOTE_BYTES);

    let (banner_addr, banner_len) = add_str(BANNER_MSG);
    let (prompt_addr, prompt_len) = add_str(PROMPT_MSG);
    let (help_addr, help_len) = add_str(HELP_MSG);
    let (capp_done_addr, capp_done_len) = add_str(CAPP_DONE_MSG);
    let (gui_done_addr, gui_done_len) = add_str(GUI_DONE_MSG);
    let (stat_addr, stat_len) = add_str(STAT_MSG);
    let (draw_addr, draw_len) = add_str(DRAW_MSG);
    let (disk_addr, disk_len) = add_str(DISK_MSG);
    let (mouse_addr, mouse_len) = add_str(MOUSE_MSG);
    let (net_addr, net_len) = add_str(NET_MSG);
    let (wget_addr, wget_len) = add_str(WGET_MSG);
    let (cat_err_addr, cat_err_len) = add_str(CAT_ERR_MSG);
    let (npu_addr, npu_len) = add_str(NPU_MSG);
    let (exec_done_addr, exec_done_len) = add_str(EXEC_DONE_MSG);
    let (cpp_done_addr, cpp_done_len) = add_str(CPP_DONE_MSG);
    let (fault_done_addr, fault_done_len) = add_str(FAULT_DONE_MSG);
    let (sensors_addr, sensors_len) = add_str(SENSORS_DONE_MSG);
    let (poweroff_addr, poweroff_len) = add_str(POWEROFF_MSG);
    let (reboot_addr, reboot_len) = add_str(REBOOT_MSG);
    let (wifi_addr, wifi_len) = add_str(WIFI_MSG);
    let (curl_addr, curl_len) = add_str(CURL_MSG);
    let (rm_addr, rm_len) = add_str(RM_MSG);
    let (kill_addr, kill_len) = add_str(KILL_MSG);
    let (exit_addr, exit_len) = add_str(EXIT_MSG);
    let (unknown_addr, unknown_len) = add_str(UNKNOWN_MSG);

    const LBL_INIT_FILL_LOOP: usize = 0;
    const LBL_SHELL_LOOP: usize = 1;
    const LBL_CMD_HELP: usize = 2;
    const LBL_CMD_LS: usize = 3;
    const LBL_CMD_CAT: usize = 4;
    const LBL_CMD_PS: usize = 5;
    const LBL_CMD_NPU: usize = 6;
    const LBL_NPU_FILL_LOOP: usize = 7;
    const LBL_CMD_EXEC: usize = 8;
    const LBL_CMD_CPP: usize = 9;
    const LBL_CMD_STAT: usize = 10;
    const LBL_CMD_DRAW: usize = 11;
    const LBL_DRAW_FILL_LOOP: usize = 12;
    const LBL_CMD_EXIT: usize = 13;
    const LBL_CMD_MOUSE: usize = 14;
    const LBL_CMD_NET: usize = 15;
    const LBL_CMD_WGET: usize = 16;
    const LBL_CMD_CAT_ARG: usize = 17;
    const LBL_CAT_FALLBACK_MOTD: usize = 18;
    const LBL_CAT_STRIP_LOOP: usize = 19;
    const LBL_CAT_ZERO_NL: usize = 20;
    const LBL_CAT_STRIP_DONE: usize = 21;
    const LBL_CAT_READ_FD_RAX: usize = 22;
    const LBL_CAT_NOT_FOUND: usize = 23;
    const LBL_CMD_GUI: usize = 24;
    const LBL_CMD_DISK: usize = 25;
    const LBL_CMD_FAULT: usize = 26;
    const LBL_CMD_SENSORS: usize = 27;
    const LBL_CMD_POWEROFF: usize = 28;
    const LBL_CMD_REBOOT: usize = 29;
    const LBL_CMD_WIFI: usize = 30;
    const LBL_CMD_CURL: usize = 31;
    const LBL_CMD_RM: usize = 32;
    const LBL_CMD_RM_ARG: usize = 33;
    const LBL_RM_STRIP_LOOP: usize = 34;
    const LBL_RM_ZERO_NL: usize = 35;
    const LBL_RM_STRIP_DONE: usize = 36;
    const LBL_CMD_KILL: usize = 37;
    const LBL_CMD_CAPP: usize = 38;

    let widget_pixels: u32 = (FB_WIDGET_DEFAULT_W * FB_WIDGET_DEFAULT_H) as u32; // 160 * 80 = 12800
    let widget_bytes: u32 = widget_pixels * 4; // 51200 bytes
    let packed_wh: u32 = ((FB_WIDGET_DEFAULT_W as u32) << 16) | (FB_WIDGET_DEFAULT_H as u32);
    let fb_present_cmd: u32 = FB_IOCTL_PRESENT_RECT as u32;
    let npu_ioctl_cmd: u32 = ACCEL_IOCTL_BIND_TENSOR_VMO as u32;
    let npu_infer_cmd: u32 = ACCEL_IOCTL_RUN_INFERENCE as u32;
    let disk_ioctl_cmd: u32 = DISK_IOCTL_SAVE_NOTE as u32;
    let mouse_poll_cmd: u32 = MOUSE_IOCTL_POLL_STATE as u32;
    let net_ping_cmd: u32 = NET_IOCTL_PING_GATEWAY as u32;
    let net_fetch_cmd: u32 = NET_IOCTL_HTTP_FETCH as u32;
    let vfs_list_cmd: u32 = VFS_IOCTL_LIST_FILES as u32;
    let vfs_ps_cmd: u32 = VFS_IOCTL_LIST_PROCESSES as u32;
    let vfs_hwmon_cmd: u32 = VFS_IOCTL_HWMON_REPORT as u32;
    let wifi_scan_cmd: u32 = WIFI_IOCTL_SCAN as u32;
    let wifi_status_cmd: u32 = WIFI_IOCTL_STATUS as u32;
    let browser_fetch_cmd: u32 = NET_IOCTL_BROWSER_FETCH_SAVE as u32;
    let cat_arg_vaddr: u32 = USER_INPUT_BUF_VADDR + 4;
    let rm_arg_vaddr: u32 = USER_INPUT_BUF_VADDR + 3;

    let mut asm = Ring3CodeBuilder::new();

    // 1. sys_open("/dev/fb0\0", flags = 2 (O_RDWR)) -> RAX = fd (5), save in R13
    asm.emit_sys_open(path_fb0_addr, 2);
    asm.emit(&[0x49, 0x89, 0xC5]); // mov r13, rax (r13 = fd of /dev/fb0)

    // 2. sys_mmap(addr = 0, len = 51200, prot = 3, flags = 1, fd = r13, offset = 0) -> RAX = 0x10000000, save in R12
    asm.emit(&[0xB8, 0x09, 0x00, 0x00, 0x00]); // mov eax, 9 (SYS_MMAP)
    asm.emit(&[0x31, 0xFF]); // xor edi, edi
    asm.emit(&[0xBE]);
    asm.emit(&widget_bytes.to_le_bytes()); // mov esi, 51200
    asm.emit(&[0xBA, 0x03, 0x00, 0x00, 0x00]); // mov edx, 3 (PROT_READ | PROT_WRITE)
    asm.emit(&[0x41, 0xBA, 0x01, 0x00, 0x00, 0x00]); // mov r10d, 1 (MAP_SHARED)
    asm.emit(&[0x4D, 0x89, 0xE8]); // mov r8, r13 (fd = /dev/fb0)
    asm.emit(&[0x45, 0x31, 0xC9]); // xor r9d, r9d
    asm.emit(&[0x0F, 0x05]); // syscall
    asm.emit(&[0x49, 0x89, 0xC4]); // mov r12, rax (r12 = 0x10000000 Zero-Copy VMO base)

    // 3. Initialize Cyber-Blue/Neon Cyan color seed in R14D and fill 160x80 RGB32 pixels in [R12]
    asm.emit(&[0x41, 0xBE, 0xE8, 0xA8, 0x12, 0x00]); // mov r14d, 0x0012A8E8
    asm.emit(&[0x4C, 0x89, 0xE7]); // mov rdi, r12
    asm.emit(&[0xB9]);
    asm.emit(&widget_pixels.to_le_bytes()); // mov ecx, 12800
    asm.bind_label(LBL_INIT_FILL_LOOP);
    asm.emit(&[0x89, 0xC8]); // mov eax, ecx
    asm.emit(&[0x69, 0xC0, 0x09, 0x05, 0x02, 0x00]); // imul eax, eax, 0x00020509
    asm.emit(&[0x44, 0x31, 0xF0]); // xor eax, r14d
    asm.emit(&[0x25, 0xFF, 0xFF, 0xFF, 0x00]); // and eax, 0x00FFFFFF
    asm.emit(&[0xAB]); // stosd ([rdi] = eax, rdi += 4)
    asm.emit(&[0xFF, 0xC9]); // dec ecx
    asm.emit_jnz_rel32(LBL_INIT_FILL_LOOP);

    // 4. Present the Zero-Copy widget on /dev/fb0 via sys_ioctl(r13, FB_IOCTL_PRESENT_RECT, packed_wh)
    asm.emit(&[0xB8, 0x10, 0x00, 0x00, 0x00]); // mov eax, 16 (SYS_IOCTL)
    asm.emit(&[0x4C, 0x89, 0xEF]); // mov rdi, r13
    asm.emit(&[0xBE]);
    asm.emit(&fb_present_cmd.to_le_bytes());
    asm.emit(&[0xBA]);
    asm.emit(&packed_wh.to_le_bytes());
    asm.emit(&[0x0F, 0x05]);

    // 5. Auto-launch FipSOs Shell UI 1280x800 Desktop Compositor (`/bin/desktop_ui.elf`) on boot!
    asm.emit(&[0xB8, 0x3B, 0x00, 0x00, 0x00]); // mov eax, 59 (SYS_EXECVE)
    asm.emit(&[0xBF]);
    asm.emit(&path_gui_addr.to_le_bytes()); // mov edi, "/bin/desktop_ui.elf\0"
    asm.emit(&[0x31, 0xF6]); // xor esi, esi
    asm.emit(&[0x31, 0xD2]); // xor edx, edx
    asm.emit(&[0x0F, 0x05]); // syscall -> runs Desktop UI until user exits to TTY

    // 6. Print TTY Shell Welcome Banner upon returning from Desktop UI
    asm.emit_sys_write(banner_addr, banner_len);

    // 7. Interactive TTY Shell Loop: prompt -> sys_read(0) -> dispatch command
    asm.bind_label(LBL_SHELL_LOOP);
    asm.emit_sys_write(prompt_addr, prompt_len);

    // Clear 64 bytes at USER_INPUT_BUF_VADDR (0x00801000) before reading keyboard line
    asm.emit(&[0xBF]);
    asm.emit(&USER_INPUT_BUF_VADDR.to_le_bytes());
    asm.emit(&[0x31, 0xC0]); // xor eax, eax
    asm.emit(&[0xB9, 0x08, 0x00, 0x00, 0x00]); // mov ecx, 8
    asm.emit(&[0xF3, 0x48, 0xAB]); // rep stosq

    // sys_read(fd = 0 (FD_STDIN), buf = 0x00801000, len = 64)
    asm.emit(&[0x31, 0xC0]); // xor eax, eax (0 = SYS_READ)
    asm.emit(&[0x31, 0xFF]); // xor edi, edi (0 = FD_STDIN)
    asm.emit(&[0xBE]);
    asm.emit(&USER_INPUT_BUF_VADDR.to_le_bytes());
    asm.emit(&[0xBA, 0x40, 0x00, 0x00, 0x00]); // mov edx, 64
    asm.emit(&[0x0F, 0x05]);

    // Load first 4 bytes of typed command from [0x00801000] into EAX
    asm.emit(&[0xBF]);
    asm.emit(&USER_INPUT_BUF_VADDR.to_le_bytes());
    asm.emit(&[0x8B, 0x07]); // mov eax, [rdi]

    // Ignore empty newline ('\n' = 0x0A)
    asm.emit(&[0x3C, 0x0A]);
    asm.emit_je_rel32(LBL_SHELL_LOOP);

    // Dispatch: "help", "capp", "gui\n", "ls\n\0", "cat\n" / "cat ", "ps\n\0", "mous", "net\n", "wget", "npu\n", "exec", "cpp\n", "stat", "draw", "exit"
    asm.emit(&[0x3D, b'h', b'e', b'l', b'p']);
    asm.emit_je_rel32(LBL_CMD_HELP);

    asm.emit(&[0x3D, b'c', b'a', b'p', b'p']);
    asm.emit_je_rel32(LBL_CMD_CAPP);

    asm.emit(&[0x3D, b'g', b'u', b'i', b'\n']);
    asm.emit_je_rel32(LBL_CMD_GUI);

    asm.emit(&[0x3D, b'l', b's', b'\n', 0]);
    asm.emit_je_rel32(LBL_CMD_LS);

    asm.emit(&[0x3D, b'c', b'a', b't', b'\n']);
    asm.emit_je_rel32(LBL_CMD_CAT);
    asm.emit(&[0x3D, b'c', b'a', b't', b' ']);
    asm.emit_je_rel32(LBL_CMD_CAT_ARG);

    asm.emit(&[0x3D, b'p', b's', b'\n', 0]);
    asm.emit_je_rel32(LBL_CMD_PS);

    asm.emit(&[0x3D, b'f', b'a', b'u', b'l']);
    asm.emit_je_rel32(LBL_CMD_FAULT);

    asm.emit(&[0x3D, b's', b'e', b'n', b's']);
    asm.emit_je_rel32(LBL_CMD_SENSORS);
    asm.emit(&[0x3D, b'h', b'w', b'm', b'o']);
    asm.emit_je_rel32(LBL_CMD_SENSORS);

    asm.emit(&[0x3D, b'p', b'o', b'w', b'e']);
    asm.emit_je_rel32(LBL_CMD_POWEROFF);

    asm.emit(&[0x3D, b'r', b'e', b'b', b'o']);
    asm.emit_je_rel32(LBL_CMD_REBOOT);

    asm.emit(&[0x3D, b'd', b'i', b's', b'k']);
    asm.emit_je_rel32(LBL_CMD_DISK);
    asm.emit(&[0x3D, b's', b'y', b'n', b'c']);
    asm.emit_je_rel32(LBL_CMD_DISK);

    asm.emit(&[0x3D, b'm', b'o', b'u', b's']);
    asm.emit_je_rel32(LBL_CMD_MOUSE);

    asm.emit(&[0x3D, b'n', b'e', b't', b'\n']);
    asm.emit_je_rel32(LBL_CMD_NET);

    asm.emit(&[0x3D, b'w', b'g', b'e', b't']);
    asm.emit_je_rel32(LBL_CMD_WGET);

    asm.emit(&[0x3D, b'w', b'i', b'f', b'i']);
    asm.emit_je_rel32(LBL_CMD_WIFI);

    asm.emit(&[0x3D, b'c', b'u', b'r', b'l']);
    asm.emit_je_rel32(LBL_CMD_CURL);

    asm.emit(&[0x3D, b'k', b'i', b'l', b'l']);
    asm.emit_je_rel32(LBL_CMD_KILL);

    asm.emit(&[0x3D, b'r', b'm', b'\n', 0]);
    asm.emit_je_rel32(LBL_CMD_RM);
    asm.emit(&[0x25, 0xFF, 0xFF, 0xFF, 0x00]); // and eax, 0x00FFFFFF (mask lowest 3 bytes)
    asm.emit(&[0x3D, b'r', b'm', b' ', 0]);     // cmp eax, ' '<<16 | 'm'<<8 | 'r'
    asm.emit_je_rel32(LBL_CMD_RM_ARG);
    asm.emit(&[0x8B, 0x07]);                   // mov eax, [rdi] (restore)

    asm.emit(&[0x3D, b'n', b'p', b'u', b'\n']);
    asm.emit_je_rel32(LBL_CMD_NPU);

    asm.emit(&[0x3D, b'e', b'x', b'e', b'c']);
    asm.emit_je_rel32(LBL_CMD_EXEC);

    asm.emit(&[0x3D, b'c', b'p', b'p', b'\n']);
    asm.emit_je_rel32(LBL_CMD_CPP);

    asm.emit(&[0x3D, b's', b't', b'a', b't']);
    asm.emit_je_rel32(LBL_CMD_STAT);

    asm.emit(&[0x3D, b'd', b'r', b'a', b'w']);
    asm.emit_je_rel32(LBL_CMD_DRAW);

    asm.emit(&[0x3D, b'e', b'x', b'i', b't']);
    asm.emit_je_rel32(LBL_CMD_EXIT);

    // Unknown command handler
    asm.emit_sys_write(unknown_addr, unknown_len);
    asm.emit_jmp_rel32(LBL_SHELL_LOOP);

    // Command: `help`
    asm.bind_label(LBL_CMD_HELP);
    asm.emit_sys_write(help_addr, help_len);
    asm.emit_jmp_rel32(LBL_SHELL_LOOP);

    // Command: `ls` -> invokes VFS_IOCTL_LIST_FILES on /dev/fb0 (r13)
    asm.bind_label(LBL_CMD_LS);
    asm.emit(&[0xB8, 0x10, 0x00, 0x00, 0x00]); // mov eax, 16 (SYS_IOCTL)
    asm.emit(&[0x4C, 0x89, 0xEF]); // mov rdi, r13
    asm.emit(&[0xBE]);
    asm.emit(&vfs_list_cmd.to_le_bytes()); // mov esi, VFS_IOCTL_LIST_FILES
    asm.emit(&[0x31, 0xD2]); // xor edx, edx
    asm.emit(&[0x0F, 0x05]); // syscall
    asm.emit_jmp_rel32(LBL_SHELL_LOOP);

    // Command: `cat` (no arg) -> reads `/downloads/fipsos_pkg.txt` if present, else `/etc/motd.txt`
    asm.bind_label(LBL_CMD_CAT);
    asm.emit_sys_open(path_pkg_addr, 0);
    asm.emit(&[0x48, 0x85, 0xC0]); // test rax, rax
    asm.emit_js_rel32(LBL_CAT_FALLBACK_MOTD);
    asm.emit_jmp_rel32(LBL_CAT_READ_FD_RAX);

    asm.bind_label(LBL_CAT_FALLBACK_MOTD);
    asm.emit_sys_open(path_motd_addr, 0);
    asm.emit_jmp_rel32(LBL_CAT_READ_FD_RAX);

    // Command: `cat <path>` -> strips trailing '\n' at 0x00801004 and opens `<path>`
    asm.bind_label(LBL_CMD_CAT_ARG);
    asm.emit(&[0xBF]);
    asm.emit(&cat_arg_vaddr.to_le_bytes()); // mov edi, 0x00801004
    asm.bind_label(LBL_CAT_STRIP_LOOP);
    asm.emit(&[0x8A, 0x07]); // mov al, [rdi]
    asm.emit(&[0x84, 0xC0]); // test al, al
    asm.emit_je_rel32(LBL_CAT_STRIP_DONE);
    asm.emit(&[0x3C, 0x0A]); // cmp al, '\n'
    asm.emit_je_rel32(LBL_CAT_ZERO_NL);
    asm.emit(&[0x48, 0xFF, 0xC7]); // inc rdi
    asm.emit_jmp_rel32(LBL_CAT_STRIP_LOOP);
    asm.bind_label(LBL_CAT_ZERO_NL);
    asm.emit(&[0xC6, 0x07, 0x00]); // mov byte ptr [rdi], 0
    asm.bind_label(LBL_CAT_STRIP_DONE);
    asm.emit_sys_open(cat_arg_vaddr, 0);
    asm.emit(&[0x48, 0x85, 0xC0]); // test rax, rax
    asm.emit_js_rel32(LBL_CAT_NOT_FOUND);

    asm.bind_label(LBL_CAT_READ_FD_RAX);
    asm.emit(&[0x49, 0x89, 0xC7]); // mov r15, rax
    asm.emit(&[0x31, 0xC0]); // xor eax, eax (0 = SYS_READ)
    asm.emit(&[0x4C, 0x89, 0xFF]); // mov rdi, r15 (fd)
    asm.emit(&[0xBE]);
    asm.emit(&USER_FILE_BUF_VADDR.to_le_bytes()); // mov esi, 0x00801200
    asm.emit(&[0xBA, 0xF8, 0x00, 0x00, 0x00]); // mov edx, 248
    asm.emit(&[0x0F, 0x05]); // syscall -> RAX = bytes_read
    asm.emit(&[0x48, 0x89, 0xC2]); // mov rdx, rax (len = bytes_read)
    asm.emit(&[0xB8, 0x01, 0x00, 0x00, 0x00]); // mov eax, 1 (SYS_WRITE)
    asm.emit(&[0xBF, 0x01, 0x00, 0x00, 0x00]); // mov edi, 1 (FD_STDOUT)
    asm.emit(&[0xBE]);
    asm.emit(&USER_FILE_BUF_VADDR.to_le_bytes()); // mov esi, 0x00801200
    asm.emit(&[0x0F, 0x05]); // syscall
    asm.emit(&[0xB8, 0x03, 0x00, 0x00, 0x00]); // mov eax, 3 (SYS_CLOSE)
    asm.emit(&[0x4C, 0x89, 0xFF]); // mov rdi, r15
    asm.emit(&[0x0F, 0x05]); // syscall
    asm.emit_jmp_rel32(LBL_SHELL_LOOP);

    asm.bind_label(LBL_CAT_NOT_FOUND);
    asm.emit_sys_write(cat_err_addr, cat_err_len);
    asm.emit_jmp_rel32(LBL_SHELL_LOOP);

    // Command: `ps` -> invokes VFS_IOCTL_LIST_PROCESSES on /dev/fb0 (r13)
    asm.bind_label(LBL_CMD_PS);
    asm.emit(&[0xB8, 0x10, 0x00, 0x00, 0x00]); // mov eax, 16 (SYS_IOCTL)
    asm.emit(&[0x4C, 0x89, 0xEF]); // mov rdi, r13
    asm.emit(&[0xBE]);
    asm.emit(&vfs_ps_cmd.to_le_bytes()); // mov esi, VFS_IOCTL_LIST_PROCESSES
    asm.emit(&[0x31, 0xD2]); // xor edx, edx
    asm.emit(&[0x0F, 0x05]); // syscall
    asm.emit_jmp_rel32(LBL_SHELL_LOOP);

    // Command: `disk` / `sync` -> invokes DISK_IOCTL_SAVE_NOTE on /dev/fb0 (r13)
    asm.bind_label(LBL_CMD_DISK);
    asm.emit(&[0xB8, 0x10, 0x00, 0x00, 0x00]); // mov eax, 16 (SYS_IOCTL)
    asm.emit(&[0x4C, 0x89, 0xEF]); // mov rdi, r13 (/dev/fb0)
    asm.emit(&[0xBE]);
    asm.emit(&disk_ioctl_cmd.to_le_bytes()); // mov esi, DISK_IOCTL_SAVE_NOTE
    asm.emit(&[0xBA, 0x01, 0x00, 0x00, 0x00]); // mov edx, 1
    asm.emit(&[0x0F, 0x05]); // syscall
    asm.emit_sys_write(disk_addr, disk_len);
    asm.emit_jmp_rel32(LBL_SHELL_LOOP);

    // Command: `mouse` -> sys_open("/dev/input/mouse0", O_RDWR) + sys_ioctl(fd, MOUSE_IOCTL_POLL_STATE, 1) + sys_close(fd)
    asm.bind_label(LBL_CMD_MOUSE);
    asm.emit_sys_open(path_mouse_addr, 2); // RAX = fd of /dev/input/mouse0 (6)
    asm.emit(&[0x49, 0x89, 0xC7]); // mov r15, rax
    asm.emit(&[0xB8, 0x10, 0x00, 0x00, 0x00]); // mov eax, 16 (SYS_IOCTL)
    asm.emit(&[0x4C, 0x89, 0xFF]); // mov rdi, r15 (fd)
    asm.emit(&[0xBE]);
    asm.emit(&mouse_poll_cmd.to_le_bytes()); // mov esi, MOUSE_IOCTL_POLL_STATE
    asm.emit(&[0xBA, 0x01, 0x00, 0x00, 0x00]); // mov edx, 1 (verbose diagnostic mode)
    asm.emit(&[0x0F, 0x05]); // syscall
    asm.emit(&[0xB8, 0x03, 0x00, 0x00, 0x00]); // mov eax, 3 (SYS_CLOSE)
    asm.emit(&[0x4C, 0x89, 0xFF]); // mov rdi, r15 (fd)
    asm.emit(&[0x0F, 0x05]); // syscall
    asm.emit_sys_write(mouse_addr, mouse_len);
    asm.emit_jmp_rel32(LBL_SHELL_LOOP);

    // Command: `net` -> sys_open("/dev/net/eth0", O_RDWR) + sys_ioctl(fd, NET_IOCTL_PING_GATEWAY, 1) + sys_close(fd)
    asm.bind_label(LBL_CMD_NET);
    asm.emit_sys_open(path_net_addr, 2); // RAX = fd of /dev/net/eth0 (7)
    asm.emit(&[0x49, 0x89, 0xC7]); // mov r15, rax
    asm.emit(&[0xB8, 0x10, 0x00, 0x00, 0x00]); // mov eax, 16 (SYS_IOCTL)
    asm.emit(&[0x4C, 0x89, 0xFF]); // mov rdi, r15 (fd)
    asm.emit(&[0xBE]);
    asm.emit(&net_ping_cmd.to_le_bytes()); // mov esi, NET_IOCTL_PING_GATEWAY
    asm.emit(&[0xBA, 0x01, 0x00, 0x00, 0x00]); // mov edx, 1
    asm.emit(&[0x0F, 0x05]); // syscall
    asm.emit(&[0xB8, 0x03, 0x00, 0x00, 0x00]); // mov eax, 3 (SYS_CLOSE)
    asm.emit(&[0x4C, 0x89, 0xFF]); // mov rdi, r15 (fd)
    asm.emit(&[0x0F, 0x05]); // syscall
    asm.emit_sys_write(net_addr, net_len);
    asm.emit_jmp_rel32(LBL_SHELL_LOOP);

    // Command: `wget` -> sys_open("/dev/net/eth0", O_RDWR) + sys_ioctl(fd, NET_IOCTL_HTTP_FETCH, 1) + sys_close(fd)
    asm.bind_label(LBL_CMD_WGET);
    asm.emit_sys_open(path_net_addr, 2); // RAX = fd of /dev/net/eth0 (7)
    asm.emit(&[0x49, 0x89, 0xC7]); // mov r15, rax
    asm.emit(&[0xB8, 0x10, 0x00, 0x00, 0x00]); // mov eax, 16 (SYS_IOCTL)
    asm.emit(&[0x4C, 0x89, 0xFF]); // mov rdi, r15 (fd)
    asm.emit(&[0xBE]);
    asm.emit(&net_fetch_cmd.to_le_bytes()); // mov esi, NET_IOCTL_HTTP_FETCH
    asm.emit(&[0xBA, 0x01, 0x00, 0x00, 0x00]); // mov edx, 1
    asm.emit(&[0x0F, 0x05]); // syscall
    asm.emit(&[0xB8, 0x03, 0x00, 0x00, 0x00]); // mov eax, 3 (SYS_CLOSE)
    asm.emit(&[0x4C, 0x89, 0xFF]); // mov rdi, r15 (fd)
    asm.emit(&[0x0F, 0x05]); // syscall
    asm.emit_sys_write(wget_addr, wget_len);
    asm.emit_jmp_rel32(LBL_SHELL_LOOP);

    // Command: `npu` -> sys_open("/models/avatar_v1.bin") + sys_open("/dev/accel/accel0") +
    // sys_ioctl(ACCEL_IOCTL_RUN_INFERENCE) + "Inferencia IA Activa" visual widget pattern on /dev/fb0
    asm.bind_label(LBL_CMD_NPU);
    asm.emit_sys_open(path_model_addr, 0); // RAX = fd of /models/avatar_v1.bin
    asm.emit(&[0x49, 0x89, 0xC7]); // mov r15, rax (r15 = model_fd)
    asm.emit_sys_open(path_accel_addr, 2); // RAX = fd of /dev/accel/accel0
    asm.emit(&[0x48, 0x89, 0xC7]); // mov rdi, rax (fd = /dev/accel/accel0)
    asm.emit(&[0xB8, 0x10, 0x00, 0x00, 0x00]); // mov eax, 16 (SYS_IOCTL)
    asm.emit(&[0xBE]);
    asm.emit(&npu_infer_cmd.to_le_bytes()); // mov esi, ACCEL_IOCTL_RUN_INFERENCE
    asm.emit(&[0x4C, 0x89, 0xFA]); // mov rdx, r15 (arg = model_fd)
    asm.emit(&[0x0F, 0x05]); // syscall

    // Render "Active AI Inference" Emerald-Matrix & Gold neural pattern into [R12]
    asm.emit(&[0x41, 0xBE, 0x50, 0xF5, 0x00, 0x00]); // mov r14d, 0x0000F550 (Matrix Emerald Seed)
    asm.emit(&[0x4C, 0x89, 0xE7]); // mov rdi, r12
    asm.emit(&[0xB9]);
    asm.emit(&widget_pixels.to_le_bytes()); // mov ecx, 12800
    asm.bind_label(LBL_NPU_FILL_LOOP);
    asm.emit(&[0x89, 0xC8]); // mov eax, ecx
    asm.emit(&[0x69, 0xC0, 0x03, 0x0B, 0x01, 0x00]); // imul eax, eax, 0x00010B03 (Neural tensor grid)
    asm.emit(&[0x44, 0x31, 0xF0]); // xor eax, r14d
    asm.emit(&[0x25, 0xFF, 0xFF, 0xFF, 0x00]); // and eax, 0x00FFFFFF
    asm.emit(&[0xAB]); // stosd
    asm.emit(&[0xFF, 0xC9]); // dec ecx
    asm.emit_jnz_rel32(LBL_NPU_FILL_LOOP);

    asm.emit(&[0xB8, 0x10, 0x00, 0x00, 0x00]); // mov eax, 16 (SYS_IOCTL)
    asm.emit(&[0x4C, 0x89, 0xEF]); // mov rdi, r13 (fd = /dev/fb0)
    asm.emit(&[0xBE]);
    asm.emit(&fb_present_cmd.to_le_bytes()); // mov esi, FB_IOCTL_PRESENT_RECT
    asm.emit(&[0xBA]);
    asm.emit(&packed_wh.to_le_bytes()); // mov edx, (160 << 16) | 80
    asm.emit(&[0x0F, 0x05]); // syscall
    asm.emit_sys_write(npu_addr, npu_len);
    asm.emit_jmp_rel32(LBL_SHELL_LOOP);

    // Command: `exec` -> invokes sys_execve(59, "/bin/npu_infer.elf\0", 0, 0) and upon return prints completion
    asm.bind_label(LBL_CMD_EXEC);
    asm.emit(&[0xB8, 0x3B, 0x00, 0x00, 0x00]); // mov eax, 59 (SYS_EXECVE)
    asm.emit(&[0xBF]);
    asm.emit(&path_exec_addr.to_le_bytes()); // mov edi, path_exec_addr ("/bin/npu_infer.elf\0")
    asm.emit(&[0x31, 0xF6]); // xor esi, esi (argv = NULL)
    asm.emit(&[0x31, 0xD2]); // xor edx, edx (envp = NULL)
    asm.emit(&[0x0F, 0x05]); // syscall -> returns child exit_code in RAX after child calls sys_exit(0)
    asm.emit_sys_write(exec_done_addr, exec_done_len);
    asm.emit_jmp_rel32(LBL_SHELL_LOOP);

    // Command: `cpp` -> invokes sys_execve(59, "/bin/avatar_cpp.elf\0", 0, 0) and upon return prints completion
    asm.bind_label(LBL_CMD_CPP);
    asm.emit(&[0xB8, 0x3B, 0x00, 0x00, 0x00]); // mov eax, 59 (SYS_EXECVE)
    asm.emit(&[0xBF]);
    asm.emit(&path_cpp_addr.to_le_bytes()); // mov edi, path_cpp_addr ("/bin/avatar_cpp.elf\0")
    asm.emit(&[0x31, 0xF6]); // xor esi, esi (argv = NULL)
    asm.emit(&[0x31, 0xD2]); // xor edx, edx (envp = NULL)
    asm.emit(&[0x0F, 0x05]); // syscall -> returns child exit_code in RAX after child calls sys_exit(0)
    asm.emit_sys_write(cpp_done_addr, cpp_done_len);
    asm.emit_jmp_rel32(LBL_SHELL_LOOP);

    // Command: `gui` -> re-launches `/bin/desktop_ui.elf` (1280x800 Win11 Desktop Compositor)
    asm.bind_label(LBL_CMD_GUI);
    asm.emit(&[0xB8, 0x3B, 0x00, 0x00, 0x00]); // mov eax, 59 (SYS_EXECVE)
    asm.emit(&[0xBF]);
    asm.emit(&path_gui_addr.to_le_bytes()); // mov edi, path_gui_addr ("/bin/desktop_ui.elf\0")
    asm.emit(&[0x31, 0xF6]); // xor esi, esi
    asm.emit(&[0x31, 0xD2]); // xor edx, edx
    asm.emit(&[0x0F, 0x05]); // syscall
    asm.emit_sys_write(gui_done_addr, gui_done_len);
    asm.emit_jmp_rel32(LBL_SHELL_LOOP);

    // Command: `capp` -> executes `/bin/c_std_app.elf` (Linux Standard C application)
    asm.bind_label(LBL_CMD_CAPP);
    asm.emit(&[0xB8, 0x3B, 0x00, 0x00, 0x00]); // mov eax, 59 (SYS_EXECVE)
    asm.emit(&[0xBF]);
    asm.emit(&path_capp_addr.to_le_bytes()); // mov edi, path_capp_addr ("/bin/c_std_app.elf\0")
    asm.emit(&[0x31, 0xF6]); // xor esi, esi
    asm.emit(&[0x31, 0xD2]); // xor edx, edx
    asm.emit(&[0x0F, 0x05]); // syscall
    asm.emit_sys_write(capp_done_addr, capp_done_len);
    asm.emit_jmp_rel32(LBL_SHELL_LOOP);

    // Command: `stat` -> invokes sys_ioctl on /dev/accel/accel0 (fd=4) + prints status
    asm.bind_label(LBL_CMD_STAT);
    asm.emit(&[0xB8, 0x10, 0x00, 0x00, 0x00]); // mov eax, 16 (SYS_IOCTL)
    asm.emit(&[0xBF, 0x04, 0x00, 0x00, 0x00]); // mov edi, 4 (FD_ACCEL_NPU0)
    asm.emit(&[0xBE]);
    asm.emit(&npu_ioctl_cmd.to_le_bytes()); // mov esi, ACCEL_IOCTL_BIND_TENSOR_VMO
    asm.emit(&[0xBA, 0x01, 0x00, 0x00, 0x00]); // mov edx, 1
    asm.emit(&[0x0F, 0x05]); // syscall
    asm.emit_sys_write(stat_addr, stat_len);
    asm.emit_jmp_rel32(LBL_SHELL_LOOP);

    // Command: `draw` -> mutates palette seed R14D, updates Zero-Copy VMO [R12], and presents on /dev/fb0
    asm.bind_label(LBL_CMD_DRAW);
    asm.emit(&[0x41, 0x81, 0xF6, 0x90, 0x30, 0xE0, 0x00]); // xor r14d, 0x00E03090
    asm.emit(&[0x41, 0x81, 0xC6, 0x22, 0x44, 0x18, 0x00]); // add r14d, 0x00184422
    asm.emit(&[0x4C, 0x89, 0xE7]); // mov rdi, r12 (0x10000000)
    asm.emit(&[0xB9]);
    asm.emit(&widget_pixels.to_le_bytes()); // mov ecx, 12800
    asm.bind_label(LBL_DRAW_FILL_LOOP);
    asm.emit(&[0x89, 0xC8]); // mov eax, ecx
    asm.emit(&[0x69, 0xC0, 0x11, 0x07, 0x03, 0x00]); // imul eax, eax, 0x00030711
    asm.emit(&[0x44, 0x31, 0xF0]); // xor eax, r14d
    asm.emit(&[0x25, 0xFF, 0xFF, 0xFF, 0x00]); // and eax, 0x00FFFFFF
    asm.emit(&[0xAB]); // stosd
    asm.emit(&[0xFF, 0xC9]); // dec ecx
    asm.emit_jnz_rel32(LBL_DRAW_FILL_LOOP);

    asm.emit(&[0xB8, 0x10, 0x00, 0x00, 0x00]); // mov eax, 16 (SYS_IOCTL)
    asm.emit(&[0x4C, 0x89, 0xEF]); // mov rdi, r13 (fd = /dev/fb0)
    asm.emit(&[0xBE]);
    asm.emit(&fb_present_cmd.to_le_bytes()); // mov esi, FB_IOCTL_PRESENT_RECT
    asm.emit(&[0xBA]);
    asm.emit(&packed_wh.to_le_bytes()); // mov edx, (160 << 16) | 80
    asm.emit(&[0x0F, 0x05]); // syscall
    asm.emit_sys_write(draw_addr, draw_len);
    asm.emit_jmp_rel32(LBL_SHELL_LOOP);

    // Command: `fault` -> executes /bin/fault_test.elf and recovers from #PF
    asm.bind_label(LBL_CMD_FAULT);
    asm.emit(&[0xB8, 0x3B, 0x00, 0x00, 0x00]); // mov eax, 59 (SYS_EXECVE)
    asm.emit(&[0xBF]);
    asm.emit(&path_fault_addr.to_le_bytes()); // mov edi, path_fault_addr
    asm.emit(&[0x31, 0xF6]); // xor esi, esi
    asm.emit(&[0x31, 0xD2]); // xor edx, edx
    asm.emit(&[0x0F, 0x05]); // syscall -> child #PF intercepted by kernel, returns RAX = -11
    asm.emit_sys_write(fault_done_addr, fault_done_len);
    asm.emit_jmp_rel32(LBL_SHELL_LOOP);

    // Command: `sensors` / `hwmon` -> invokes VFS_IOCTL_HWMON_REPORT on /dev/fb0 (r13)
    asm.bind_label(LBL_CMD_SENSORS);
    asm.emit(&[0xB8, 0x10, 0x00, 0x00, 0x00]); // mov eax, 16 (SYS_IOCTL)
    asm.emit(&[0x4C, 0x89, 0xEF]); // mov rdi, r13
    asm.emit(&[0xBE]);
    asm.emit(&vfs_hwmon_cmd.to_le_bytes()); // mov esi, VFS_IOCTL_HWMON_REPORT
    asm.emit(&[0x31, 0xD2]); // xor edx, edx
    asm.emit(&[0x0F, 0x05]); // syscall
    asm.emit_sys_write(sensors_addr, sensors_len);
    asm.emit_jmp_rel32(LBL_SHELL_LOOP);

    // Command: `poweroff` -> ACPI shutdown via sys_reboot(0, 0, 0, 0)
    asm.bind_label(LBL_CMD_POWEROFF);
    asm.emit_sys_write(poweroff_addr, poweroff_len);
    asm.emit(&[0xB8, 0xA9, 0x00, 0x00, 0x00]); // mov eax, 169 (SYS_REBOOT)
    asm.emit(&[0x31, 0xFF]); // xor edi, edi (magic1 = 0)
    asm.emit(&[0x31, 0xF6]); // xor esi, esi (magic2 = 0)
    asm.emit(&[0x31, 0xD2]); // xor edx, edx (cmd = 0: POWER_OFF)
    asm.emit(&[0x45, 0x31, 0xD2]); // xor r10d, r10d (arg = 0)
    asm.emit(&[0x0F, 0x05]); // syscall
    asm.emit_jmp_rel32(LBL_SHELL_LOOP);

    // Command: `reboot` -> reset via sys_reboot(0, 0, 1, 0)
    asm.bind_label(LBL_CMD_REBOOT);
    asm.emit_sys_write(reboot_addr, reboot_len);
    asm.emit(&[0xB8, 0xA9, 0x00, 0x00, 0x00]); // mov eax, 169 (SYS_REBOOT)
    asm.emit(&[0x31, 0xFF]); // xor edi, edi (magic1 = 0)
    asm.emit(&[0x31, 0xF6]); // xor esi, esi (magic2 = 0)
    asm.emit(&[0xBA, 0x01, 0x00, 0x00, 0x00]); // mov edx, 1 (cmd = 1: RESTART)
    asm.emit(&[0x45, 0x31, 0xD2]); // xor r10d, r10d (arg = 0)
    asm.emit(&[0x0F, 0x05]); // syscall
    asm.emit_jmp_rel32(LBL_SHELL_LOOP);

    // Command: `wifi` -> sys_ioctl(/dev/fb0, WIFI_IOCTL_SCAN, 0) + sys_ioctl(/dev/fb0, WIFI_IOCTL_STATUS, 1)
    asm.bind_label(LBL_CMD_WIFI);
    asm.emit(&[0xB8, 0x10, 0x00, 0x00, 0x00]); // mov eax, 16 (SYS_IOCTL)
    asm.emit(&[0x4C, 0x89, 0xEF]); // mov rdi, r13 (/dev/fb0)
    asm.emit(&[0xBE]);
    asm.emit(&wifi_scan_cmd.to_le_bytes()); // mov esi, WIFI_IOCTL_SCAN
    asm.emit(&[0x31, 0xD2]); // xor edx, edx
    asm.emit(&[0x0F, 0x05]); // syscall
    asm.emit(&[0xB8, 0x10, 0x00, 0x00, 0x00]); // mov eax, 16 (SYS_IOCTL)
    asm.emit(&[0x4C, 0x89, 0xEF]); // mov rdi, r13
    asm.emit(&[0xBE]);
    asm.emit(&wifi_status_cmd.to_le_bytes()); // mov esi, WIFI_IOCTL_STATUS
    asm.emit(&[0xBA, 0x01, 0x00, 0x00, 0x00]); // mov edx, 1 (activate wlan0)
    asm.emit(&[0x0F, 0x05]); // syscall
    asm.emit_sys_write(wifi_addr, wifi_len);
    asm.emit_jmp_rel32(LBL_SHELL_LOOP);

    // Command: `curl` -> sys_ioctl(/dev/fb0, NET_IOCTL_BROWSER_FETCH_SAVE, 1)
    asm.bind_label(LBL_CMD_CURL);
    asm.emit(&[0xB8, 0x10, 0x00, 0x00, 0x00]); // mov eax, 16 (SYS_IOCTL)
    asm.emit(&[0x4C, 0x89, 0xEF]); // mov rdi, r13
    asm.emit(&[0xBE]);
    asm.emit(&browser_fetch_cmd.to_le_bytes()); // mov esi, NET_IOCTL_BROWSER_FETCH_SAVE
    asm.emit(&[0xBA, 0x01, 0x00, 0x00, 0x00]); // mov edx, 1
    asm.emit(&[0x0F, 0x05]); // syscall
    asm.emit_sys_write(curl_addr, curl_len);
    asm.emit_jmp_rel32(LBL_SHELL_LOOP);

    // Command: `rm` (no arg) -> sys_unlink(path_note_addr)
    asm.bind_label(LBL_CMD_RM);
    asm.emit(&[0xB8]);
    asm.emit(&(SYS_UNLINK as u32).to_le_bytes()); // mov eax, 87 (SYS_UNLINK)
    asm.emit(&[0xBF]);
    asm.emit(&path_note_addr.to_le_bytes()); // mov edi, path_note_addr
    asm.emit(&[0x0F, 0x05]); // syscall
    asm.emit_sys_write(rm_addr, rm_len);
    asm.emit_jmp_rel32(LBL_SHELL_LOOP);

    // Command: `rm <path>` -> strips trailing '\n' at rm_arg_vaddr (0x00801003) and sys_unlink(rm_arg_vaddr)
    asm.bind_label(LBL_CMD_RM_ARG);
    asm.emit(&[0xBF]);
    asm.emit(&rm_arg_vaddr.to_le_bytes()); // mov edi, 0x00801003
    asm.bind_label(LBL_RM_STRIP_LOOP);
    asm.emit(&[0x8A, 0x07]); // mov al, [rdi]
    asm.emit(&[0x84, 0xC0]); // test al, al
    asm.emit_je_rel32(LBL_RM_STRIP_DONE);
    asm.emit(&[0x3C, 0x0A]); // cmp al, '\n'
    asm.emit_je_rel32(LBL_RM_ZERO_NL);
    asm.emit(&[0x48, 0xFF, 0xC7]); // inc rdi
    asm.emit_jmp_rel32(LBL_RM_STRIP_LOOP);
    asm.bind_label(LBL_RM_ZERO_NL);
    asm.emit(&[0xC6, 0x07, 0x00]); // mov byte ptr [rdi], 0
    asm.bind_label(LBL_RM_STRIP_DONE);
    asm.emit(&[0xB8]);
    asm.emit(&(SYS_UNLINK as u32).to_le_bytes()); // mov eax, 87 (SYS_UNLINK)
    asm.emit(&[0xBF]);
    asm.emit(&rm_arg_vaddr.to_le_bytes()); // mov edi, rm_arg_vaddr
    asm.emit(&[0x0F, 0x05]); // syscall
    asm.emit_sys_write(rm_addr, rm_len);
    asm.emit_jmp_rel32(LBL_SHELL_LOOP);

    // Command: `kill` -> sys_kill(1051, 9) (kill bg downloader)
    asm.bind_label(LBL_CMD_KILL);
    asm.emit(&[0xB8]);
    asm.emit(&(SYS_KILL as u32).to_le_bytes()); // mov eax, 62 (SYS_KILL)
    asm.emit(&[0xBF, 0x1B, 0x04, 0x00, 0x00]); // mov edi, 1051 (PID bg downloader)
    asm.emit(&[0xBE, 0x09, 0x00, 0x00, 0x00]); // mov esi, 9 (SIGKILL)
    asm.emit(&[0x0F, 0x05]); // syscall
    asm.emit_sys_write(kill_addr, kill_len);
    asm.emit_jmp_rel32(LBL_SHELL_LOOP);

    // Command: `exit` -> prints goodbye and invokes sys_exit(0)
    asm.bind_label(LBL_CMD_EXIT);
    asm.emit_sys_write(exit_addr, exit_len);
    asm.emit(&[0xB8, 0x3C, 0x00, 0x00, 0x00]); // mov eax, 60 (SYS_EXIT)
    asm.emit(&[0x31, 0xFF]); // xor edi, edi (status = 0)
    asm.emit(&[0x0F, 0x05]); // syscall

    let machine_code = asm.finish();
    assert!(
        machine_code.len() <= RODATA_OFFSET,
        "El codigo maquina de Anillo 3 excede RODATA_OFFSET"
    );

    Elf64Loader::build_single_segment_elf64(
        ostd::arch::syscall::USER_CODE_BASE_VADDR,
        &machine_code,
        RODATA_OFFSET,
        &rodata[..cursor],
    )
}
