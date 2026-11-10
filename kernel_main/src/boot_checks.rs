#![forbid(unsafe_code)]

//! FipSOs Kernel Boot Diagnostics (`kernel_main::boot_checks`).
//!
//! Executes hardware discovery and core kernel primitives verification:
//! CPUID topology, PCIe bus scanning, VMO Zero-Copy allocations, FfiJail memory isolation,
//! and Zircon Channel IPC message-passing within the asynchronous Executor.

use alloc::sync::Arc;
use alloc::vec::Vec;
use kernel_objects::{
    yield_now, Channel, Executor, HandleTable, KernelObject, MessagePacket, Rights, Vmo, VmoUsage,
};
use ostd::bus::pcie::PciDeviceClass;
use ostd::ffi_jail::{sample_c_firmware_routine, FfiJail};
use ostd::sync::SpinLock;

/// Executes complete kernel initialization diagnostics and returns the shared VMO and IPC Channel
/// allocated for Ring 3 userland processes.
pub fn run_boot_diagnostics() -> (Arc<Vmo>, Arc<Channel>) {
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

    // 2. Descubrimiento de Hardware en el Bus PCI/PCIe (GPUs Clase 0x03, NPUs Clase 0x12, USB Clase 0x0C)
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
    let usb_count = pci_devices
        .iter()
        .filter(|d| matches!(d.classification(), PciDeviceClass::UsbController { .. }))
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
        } else if let PciDeviceClass::UsbController { prog_if } = dev.classification() {
            let uname = match prog_if {
                0x30 => "xHCI USB 3.0",
                0x20 => "EHCI USB 2.0",
                0x10 => "OHCI USB 1.1",
                0x00 => "UHCI USB 1.1",
                _ => "USB Host Controller",
            };
            ostd::fb_println!(
                "[ostd::bus::pcie] >>> [USB DETECTADO] {:02x}:{:02x}.{} | Vendor={:#06x} Device={:#06x} ({}) BAR0={:#x}",
                dev.bus,
                dev.device,
                dev.function,
                dev.vendor_id,
                dev.device_id,
                uname,
                dev.bar0
            );
        }
    }
    dual_println!(
        "[FipSOs::pcie] Escaneo PCI/PCIe completado: {} dispositivos totales ({} NIC 0x02, {} GPU 0x03, {} NPU 0x12, {} USB 0x0C)",
        pci_devices.len(),
        nic_count,
        gpu_count,
        npu_count,
        usb_count
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

    (vmo, endpoint_b)
}
