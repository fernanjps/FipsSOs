#![forbid(unsafe_code)]

//! FipSOs Kernel Boot Hardware Discovery (`kernel_main::boot_checks`).
//!
//! Executes hardware discovery on CPUID topology and PCIe bus scanning.

use ostd::bus::pcie::PciDeviceClass;

/// Executes kernel hardware discovery diagnostics at boot without heavy dummy allocations.
pub fn run_boot_diagnostics() {
    #[cfg(target_arch = "x86_64")]
    {
        let msr_rep = ostd::arch::msr::probe_msr_gp_recovery_selftest();
        dual_println!(
            "[ostd::arch::msr] #GP Trap Recovery active | HWCR={} THERM={} X2APIC={} WRMSR_GP={} | Recovered #GP={}",
            msr_rep.hwcr.is_some(),
            msr_rep.therm_status.is_some(),
            msr_rep.x2apic_probe.is_some(),
            msr_rep.wrmsr_gp_ok,
            msr_rep.gp_after
        );
    }

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
}
