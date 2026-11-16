# FipSOs Architecture & Phase History (v0.1 – v6.0)

## 1. Framekernel Architecture (Layers 0 – 3)

FipSOs is a multi-architecture **Framekernel** operating system (`x86_64` primary, `aarch64` base) booted via the **Limine** protocol (`0xFFFFFFFF80000000` higher-half) with a capability-based kernel object model and a POSIX / Linux ABI personality layer.

| Layer | Crate / Directory | Privilege & Safety | Responsibility |
| :--- | :--- | :--- | :--- |
| **Layer 0 (HAL)** | `ostd/` | Ring 0 (`unsafe`, ASM, FFI isolated) | Boot, GDT/IDT/SMP, MM (Frame/Heap/VM/DMA), PCIe, ACPI MADT/FADT/EC, Storage (AHCI/NVMe), Net (e1000e/TCP/UDP/DHCP/802.11), USB xHCI, Intel HDA, Framebuffer (`fb_console`) |
| **Layer 1 (Objects)** | `kernel_objects/` | Ring 0 (`#![forbid(unsafe_code)]`) | Capability handles, `Process`, `Thread`, `Vmo`, `Channel`, `Executor` |
| **Layer 2 (ABI/VFS)** | `linux_abi/` | Ring 0 (`#![forbid(unsafe_code)]`) | Linux x86_64 syscall dispatcher, Capability VFS, TarFS, FAT32, DiskFS (SATA), ELF64 W^X loader, per-slot `ProcessFdTable`/`cwd`/`program_name` |
| **Layer 2 (Entry)** | `kernel_main/` | Ring 0 (`#![forbid(unsafe_code)]`) | Kernel entry point (`fipsos_main`), boot diagnostics, initial Ring 3 launch |
| **Layer 3 (Userland)** | `userland/`, `c_std_userland/`, `cpp_userland/`, `goose_userland/` | Ring 3 (`#![forbid(unsafe_code)]` in Rust) | `fipsos_sh`, `kilo`, `pkg`, `httpd`, `lua`, `tcc`, `busybox`, `fips_ai` (AVX2/SSE2), `desktop_ui.elf` (C++20 compositor), `avatar_cpp.elf` |

### Non-Negotiable Invariants
- Strict `#![no_std]` across all kernel crates (`core` and `alloc` only).
- `#![forbid(unsafe_code)]` enforced on `kernel_objects`, `linux_abi`, `kernel_main`, and `userland`.
- All `unsafe`, inline assembly, `.s` stubs, and C FFI drivers (`ffi_jail`) confined exclusively to `ostd`.
- Modular files strictly `< 600` lines; zero simulated hardware fallbacks in Ring 0.

---

## 2. Module Map

- **`ostd/src/arch/x86_64/`**: `gdt.rs`, `idt.rs`, `smp.rs`, `acpi_tables.rs`, `power_battery.rs`, `keyboard.rs`, `mouse.rs`, `sensors.rs`, `rtc_power.rs`, `syscall.rs`, `topology.rs`, `audio.rs`, `fpu.rs`, `serial.rs`.
- **`ostd/src/bus/`**: `pcie.rs`, `intel_hda.rs`, `firmware_loader.rs`, `usb/xhci.rs`, `usb/xhci_hid.rs`.
- **`ostd/src/storage/`**: `mod.rs`, `ahci.rs`, `nvme.rs`.
- **`ostd/src/net/`**: `mod.rs`, `e1000.rs`, `arp_ipv4.rs`, `dhcp.rs`, `tcp_client.rs`, `tcp_server.rs`, `remote_shell.rs` (IEEE 802.11 / WPA2-CCMP live in `linux_abi/src/net_80211.rs`).
- **`ostd/src/fb_console/`**: `mod.rs`, `ansi.rs`, `render.rs`, `dma.rs`, `scrollback.rs`.
- **`linux_abi/src/vfs/`**: `mod.rs`, `mount.rs`, `ops.rs`, `file_nodes.rs`, `ioctl.rs`, `fd_table.rs`, `devfs_sysfs.rs`, `dir.rs`, `pipe.rs`.
- **`linux_abi/src/syscall/`**: `mod.rs`, `fs.rs`, `fs_dir.rs`, `fd_ops.rs`, `process.rs`, `mem.rs`, `compat.rs`, `dispatch_ext.rs`, `httpd_bg.rs`, `net_sys.rs`, `sandbox.rs`, `sysinfo.rs`.

---

## 3. Phase History Summary (up to v6.0)

- **v1.x – v3.x (Foundation & POSIX):** Limine higher-half boot, Framekernel split, VMO/Channel capabilities, ELF64 W^X loader, TarFS/FAT32/DiskFS, SMP, e1000e + DHCP + TCP/HTTP, PS/2 mouse/keyboard, C11 shell & editor (`fipsos_sh`, `kilo`).
- **v4.x (Userland Ecosystem & 3D Desktop):** `busybox`, `tcc` + C sysroot, `lua`, `goose_userland` (`goose_mon`, `goose_audit`, `fips_ai`), C++20 `desktop_ui.elf` compositor with Phong 3D Z-buffer, multi-window WM, clipboard, and AI Security Center.
- **v5.0 – v5.2 (Zero-Copy Hardware & Power):** Unified `ProcessFdTable`, BMP viewer, ACPI/EC battery (`BAT0`) and power governor (`performance`, `balanced`, `powersave`).
- **v5.3 – v5.5 (Real Hardware, Zero Simulation):** Real ACPI `MADT`/`FADT` parser, AMD CPB / Intel Turbo MSR control, Intel HDA CORB/RIRB DMA (`/dev/audio`), MADT-driven SMP `INIT-SIPI-SIPI`, `AVX2` `fips_ai --stream-audit`, PCIe Wi-Fi + DMA firmware loader + IEEE 802.11 / WPA2-PSK AES-CCMP.
- **v5.6 – v5.7 (VirtualBox Hardening & Real Drivers Completion):** Panic/IRQ lock hardening (`SpinLockIrqSave`), TSC-calibrated timeouts in `time.rs`/`ahci.rs`/`xhci.rs`, NVMe `ADMIN_OPC_IDENTIFY` + I/O queues, full e1000e DHCP/ARP/ICMP/TCP, dynamic `/sys/devices/system/cpu/*` topology.
- **v5.8 (VirtualBox `/proc`+`/sys` Unblock & Process Isolation):** 100% lazy `/proc` and `/sys` mounting, hypervisor `CPUID.1:ECX[31]` MSR guard & EC `0x00`/`0xFF` fast-reject, cached PCIe bus `0..=3` scan, `flush_console()` on Ring 0 panic, per-slot `cwd`/`program_name`/`FdTable` isolation, and cooperative `wait_for_input_event()`.
- **v5.8.1 (Framebuffer Immediate Present, Quiet Console, Repo Hygiene):** Immediate first-frame `FB_IOCTL_FLIP_BUFFER` presentation in `desktop_ui.cpp` before network/NPU probes, Lua REPL `exit`/`quit` support, Ring 3 syscall trace silencing on framebuffer after boot, `build/` cleanup + `fipsos-baremetal-usb.img` refresh, and `docs/ARCHITECTURE_HISTORY.md`.
- **v5.9 (Clean Desktop, Non-Blocking Mouse, Real UEFI/E820 Scalable Memory):**
  - Clean desktop launch in `desktop_ui.cpp` (all windows `open = false` by default), immediate pre-frame `refresh_kernel_telemetry()` + mouse sync so frame 1 displays live RTC clock and real RAM, removal of blocking startup network/NPU/VFS probes, lazy per-window loading inside `focus_window()`, and clean return to desktop on closing the last window.
  - Real UEFI/E820 physical memory map parsing and logging (`ostd/src/mm/frame.rs`, 32 GiB bitmap support, `< 1 MiB` trampoline/BIOS protection, IRQ-safe `with_allocator`), dynamically scaled initial Kernel Heap + OOM `add_region()` expansion (`ostd/src/mm/heap.rs`, `ostd/src/mm/mod.rs`), raised `USER_HEAP_MAX_VADDR` (`0x4000_0000`, ~992 MiB dynamic `brk`) and `USER_MMAP_BASE_VADDR` (`0x4000_0000`), and full `sys_sysinfo` physical/heap reporting.
  - Hardened non-blocking PS/2 (`IRQ12`/`0x64`+`0x60` under `without_interrupts`, TSC-bounded 10 ms waits, 24-byte burst drain) and USB xHCI HID mouse polling.
- **v6.0 (VirtualBox Instant Window Opening, RAM VFS/Driver Caches, Non-Blocking Child Exec):**
  - Instant `< 0.1 ms` window opening via pure in-memory `focus_window()` + `open_window_instant()` (compositor repaints immediately before deferred data loading for `hwmon`, FAT32 directory listing, and Security Center).
  - Eliminated blocking VFS/FAT32/sysfs syscalls from pure render loops (`draw_taskbar`, `draw_win_monitor`, `draw_win_security`) using RAM-cached fields in `CompositorState`.
  - Tri-state ACPI EC probing (`EC_PRESENT: AtomicI8` in `ostd/src/arch/x86_64/power_battery.rs`, zero port `0x62`/`0x66` access when absent + 200-tick refresh throttle), 50-tick cached CMOS RTC (`rtc_power.rs`), 25-tick cached `/sys/fipsos/hwmon` (`devfs_sysfs.rs`), and 512-slot heap write-through RAM sector cache (`linux_abi/src/fat32/partition.rs`).
  - Added `/tmp` RAM directory and `/dev/null` VFS device node (across `sys_read`/`sys_write`/`sys_ioctl` even via `dup2`); unified Layer 3 child execution via `exec_child_capturing()` (`cpp_userland`) detaching `FD_STDIN` -> `/dev/null`, capturing logs sequentially in `/tmp/*` instead of SATA DiskFS, restoring `FB_IOCTL_SET_DESKTOP_MODE`, and keeping `avatar_cpp.elf` responsive/non-hanging.
