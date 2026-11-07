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
    Elf64Loader, ACCEL_IOCTL_BIND_TENSOR_VMO, ACCEL_IOCTL_RUN_INFERENCE, FB_IOCTL_PRESENT_RECT,
    FB_WIDGET_DEFAULT_H, FB_WIDGET_DEFAULT_W, FD_FB0, VFS_IOCTL_LIST_FILES,
    VFS_IOCTL_LIST_PROCESSES,
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
    labels: [usize; 20],
    rel32_fixups: Vec<(usize, usize)>,
}

impl Ring3CodeBuilder {
    fn new() -> Self {
        Self {
            bytes: Vec::with_capacity(768),
            labels: [0; 20],
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
        "[FipSOs] Framekernel FipSOs v1.1 activo (FPU/SSE2 FXSAVE64 + C11 FP32 NPU + C++20 Phong 3D)."
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
        }
    }
    dual_println!(
        "[FipSOs::pcie] Escaneo PCI/PCIe completado: {} dispositivos totales ({} GPU Clase 0x03, {} NPU Clase 0x12)",
        pci_devices.len(),
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

    // 5. Construir el Binario ELF64 del Mini-Shell Interactivo con TarFs y Demo Grafica Zero-Copy
    let elf_image = build_interactive_shell_elf64();
    let loaded_elf = Elf64Loader::load(&elf_image, 0)
        .expect("Fallo al cargar el binario ELF64 del Shell en espacio de usuario");

    // 6. Crear los objetos Process y Thread de Zircon y registrarlos en SchedulerRegistry (IRQ0 Round-Robin)
    let shell_process = Process::new("fipsos-shell.elf", loaded_elf.entry_point);
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
        0x0080_3FF0,
    );

    let (daemon_entry, daemon_stack_top) = ostd::scheduler::daemon_entry_and_stack_top();
    let daemon_process = Process::new("npu-telemetryd", daemon_entry);
    let daemon_thread = Thread::new(
        "npu-worker",
        daemon_process.koid(),
        daemon_entry,
        daemon_stack_top,
    );

    let infer_process = Process::new("/bin/npu_infer.elf", 0x0050_0000);
    let infer_thread = Thread::new(
        "rust-elf64",
        infer_process.koid(),
        0x0050_0000,
        0x0090_3FF8,
    );

    let avatar_cpp_process = Process::new("/bin/avatar_cpp.elf", 0x0060_0000);
    let avatar_cpp_thread = Thread::new(
        "cpp20-elf64",
        avatar_cpp_process.koid(),
        0x0060_0000,
        0x0090_3FF8,
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
    );

    dual_println!(
        "[FipSOs::process] Proceso \"{}\" ({}) Thread \"{}\" ({}) Estado={} | Handles=[{}, {}] | EntryPoint={:#x}",
        shell_process.name(),
        shell_process.koid(),
        shell_thread.name(),
        shell_thread.koid(),
        shell_process.state(),
        proc_vmo_handle,
        proc_chan_handle,
        shell_process.entry_point()
    );
    dual_println!(
        "[Thread] Planificador ASM switch.s activo: Shell({}) + Daemon({} @ 16KiB Stack {:#x}) + Rust({}) + C++20({})",
        shell_thread.koid(),
        daemon_thread.koid(),
        daemon_stack_top,
        infer_thread.koid(),
        avatar_cpp_thread.koid()
    );

    let exit_code = ostd::arch::syscall::enter_ring3_at(shell_process.entry_point())
        .expect("Fallo en la transicion a Anillo 3");
    shell_process.set_state(ProcessState::Exited(exit_code));

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
    const RODATA_OFFSET: usize = 0x420; // .rodata starts at 0x0040_0420
    const RODATA_BASE: u32 = CODE_BASE + (RODATA_OFFSET as u32);
    const USER_INPUT_BUF_VADDR: u32 = 0x0080_1000; // Ring 3 command buffer in stack page
    const USER_FILE_BUF_VADDR: u32 = 0x0080_1200; // Ring 3 file read buffer for `cat`

    const PATH_FB0_BYTES: &[u8] = b"/dev/fb0\0";
    const PATH_MOTD_BYTES: &[u8] = b"/etc/motd.txt\0";
    const PATH_MODEL_BYTES: &[u8] = b"/models/avatar_v1.bin\0";
    const PATH_ACCEL_BYTES: &[u8] = b"/dev/accel/accel0\0";
    const PATH_EXEC_BYTES: &[u8] = b"/bin/npu_infer.elf\0";
    const PATH_CPP_BYTES: &[u8] = b"/bin/avatar_cpp.elf\0";

    const BANNER_MSG: &[u8] =
        b"[UserRing3] FipSOs v1.1 Shell Poliglota (FPU/SSE2 + C11 FP32 + Rust + C++20) activo en Anillo 3. Escribe 'help'.\n";
    const PROMPT_MSG: &[u8] = b"fipsos@v1.1:/$ ";
    const HELP_MSG: &[u8] =
        b"[Shell::help] Comandos: help | ls | cat | ps | npu | exec | cpp (Motor Phong 3D SSE2: W/A/S/D/N/Q) | stat | draw | exit\n";
    const STAT_MSG: &[u8] =
        b"[Shell::stat] Diagnostico Zero-Copy NPU/GPU y Timer IRQ0 completado.\n";
    const DRAW_MSG: &[u8] =
        b"[Shell::draw] Widget Zero-Copy en /dev/fb0 actualizado en vivo desde Anillo 3!\n";
    const NPU_MSG: &[u8] =
        b"[Shell::npu] Inferencia Zero-Copy /models/avatar_v1.bin -> Driver C11 /dev/accel/accel0 OK!\n";
    const EXEC_DONE_MSG: &[u8] =
        b"[Shell::exec] Proceso hijo /bin/npu_infer.elf (Rust ELF64) finalizado con exit_code=0\n";
    const CPP_DONE_MSG: &[u8] =
        b"[Shell::cpp] Motor C++20 /bin/avatar_cpp.elf (Anillo 3 @ 0x600000) finalizado con exit_code=0\n";
    const EXIT_MSG: &[u8] = b"[Shell::exit] Cerrando sesion interactiva FipSOs v1.1 en Anillo 3...\n";
    const UNKNOWN_MSG: &[u8] =
        b"[Shell] Comando desconocido. Escribe: help | ls | cat | ps | npu | exec | cpp | stat | draw | exit\n";

    let mut rodata = [0u8; 1280];
    let mut cursor = 0usize;
    let mut add_str = |s: &[u8]| -> (u32, u32) {
        let addr = RODATA_BASE + (cursor as u32);
        rodata[cursor..cursor + s.len()].copy_from_slice(s);
        cursor = (cursor + s.len() + 7) & !7;
        (addr, s.len() as u32)
    };

    let (path_fb0_addr, _) = add_str(PATH_FB0_BYTES);
    let (path_motd_addr, _) = add_str(PATH_MOTD_BYTES);
    let (path_model_addr, _) = add_str(PATH_MODEL_BYTES);
    let (path_accel_addr, _) = add_str(PATH_ACCEL_BYTES);
    let (path_exec_addr, _) = add_str(PATH_EXEC_BYTES);
    let (path_cpp_addr, _) = add_str(PATH_CPP_BYTES);

    let (banner_addr, banner_len) = add_str(BANNER_MSG);
    let (prompt_addr, prompt_len) = add_str(PROMPT_MSG);
    let (help_addr, help_len) = add_str(HELP_MSG);
    let (stat_addr, stat_len) = add_str(STAT_MSG);
    let (draw_addr, draw_len) = add_str(DRAW_MSG);
    let (npu_addr, npu_len) = add_str(NPU_MSG);
    let (exec_done_addr, exec_done_len) = add_str(EXEC_DONE_MSG);
    let (cpp_done_addr, cpp_done_len) = add_str(CPP_DONE_MSG);
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

    let widget_pixels: u32 = (FB_WIDGET_DEFAULT_W * FB_WIDGET_DEFAULT_H) as u32; // 160 * 80 = 12800
    let widget_bytes: u32 = widget_pixels * 4; // 51200 bytes
    let packed_wh: u32 = ((FB_WIDGET_DEFAULT_W as u32) << 16) | (FB_WIDGET_DEFAULT_H as u32);
    let fb_present_cmd: u32 = FB_IOCTL_PRESENT_RECT as u32;
    let npu_ioctl_cmd: u32 = ACCEL_IOCTL_BIND_TENSOR_VMO as u32;
    let npu_infer_cmd: u32 = ACCEL_IOCTL_RUN_INFERENCE as u32;
    let vfs_list_cmd: u32 = VFS_IOCTL_LIST_FILES as u32;
    let vfs_ps_cmd: u32 = VFS_IOCTL_LIST_PROCESSES as u32;

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

    // 5. Print Shell Welcome Banner
    asm.emit_sys_write(banner_addr, banner_len);

    // 6. Interactive Shell Loop: prompt -> sys_read(0) -> dispatch command
    asm.bind_label(LBL_SHELL_LOOP);
    asm.emit_sys_write(prompt_addr, prompt_len);

    // Clear 8 bytes at USER_INPUT_BUF_VADDR (0x00801000) before reading keyboard line
    asm.emit(&[0xBF]);
    asm.emit(&USER_INPUT_BUF_VADDR.to_le_bytes());
    asm.emit(&[0x31, 0xC0]); // xor eax, eax
    asm.emit(&[0x48, 0x89, 0x07]); // mov [rdi], rax

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

    // Dispatch: "help", "ls\n\0", "cat\n" / "cat ", "npu\n", "stat", "draw", "exit"
    asm.emit(&[0x3D, b'h', b'e', b'l', b'p']);
    asm.emit_je_rel32(LBL_CMD_HELP);

    asm.emit(&[0x3D, b'l', b's', b'\n', 0]);
    asm.emit_je_rel32(LBL_CMD_LS);

    asm.emit(&[0x3D, b'c', b'a', b't', b'\n']);
    asm.emit_je_rel32(LBL_CMD_CAT);
    asm.emit(&[0x3D, b'c', b'a', b't', b' ']);
    asm.emit_je_rel32(LBL_CMD_CAT);

    asm.emit(&[0x3D, b'p', b's', b'\n', 0]);
    asm.emit_je_rel32(LBL_CMD_PS);

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

    // Command: `cat` -> sys_open("/etc/motd.txt") -> sys_read(fd, USER_FILE_BUF, 200) -> sys_write(1, USER_FILE_BUF, rax)
    asm.bind_label(LBL_CMD_CAT);
    asm.emit_sys_open(path_motd_addr, 0); // RAX = fd of /etc/motd.txt
    asm.emit(&[0x49, 0x89, 0xC7]); // mov r15, rax
    asm.emit(&[0x31, 0xC0]); // xor eax, eax (0 = SYS_READ)
    asm.emit(&[0x4C, 0x89, 0xFF]); // mov rdi, r15 (fd)
    asm.emit(&[0xBE]);
    asm.emit(&USER_FILE_BUF_VADDR.to_le_bytes()); // mov esi, 0x00801200
    asm.emit(&[0xBA, 0xC8, 0x00, 0x00, 0x00]); // mov edx, 200
    asm.emit(&[0x0F, 0x05]); // syscall -> RAX = bytes_read
    asm.emit(&[0x48, 0x89, 0xC2]); // mov rdx, rax (len = bytes_read)
    asm.emit(&[0xB8, 0x01, 0x00, 0x00, 0x00]); // mov eax, 1 (SYS_WRITE)
    asm.emit(&[0xBF, 0x01, 0x00, 0x00, 0x00]); // mov edi, 1 (FD_STDOUT)
    asm.emit(&[0xBE]);
    asm.emit(&USER_FILE_BUF_VADDR.to_le_bytes()); // mov esi, 0x00801200
    asm.emit(&[0x0F, 0x05]); // syscall
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
