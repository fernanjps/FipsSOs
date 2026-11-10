# Reglas del Proyecto FipSOs (Workspace Rules & Context)

Este archivo (`AGENTS.md`) define el contexto maestro, las reglas inquebrantables de arquitectura y las directrices de desarrollo para el agente de IA al trabajar en el repositorio **FipSOs**.

---

## 1. Visión General del Sistema Operativo

- **Nombre:** **FipSOs**
- **Arquitectura de Kernel:** **Framekernel** multi-arquitectura (`x86_64` activo, `aarch64` base).
- **Modelo de Seguridad:** Basado en **Capacidades** (inspirado en Zircon/Fuschia) con personalidades desacopladas (Capa de compatibilidad Linux ABI / POSIX).
- **Aceleración y Hardware:** Soporte nativo Zero-Copy para GPU (DRM) y NPU (`/dev/accel`).
- **Bootloader:** Protocolo **Limine** (Higher-Half Kernel en `0xFFFFFFFF80000000`).

---

## 2. Mapa de Crates y Estructura del Código

El workspace de Rust se divide estrictamente según las capas de abstracción:

```text
c:\FipsSOs\
├── ostd/               # Hardware Abstraction Layer (HAL) & Core Primitives
│   ├── src/arch/       # x86_64 y aarch64 (GDT, IDT, SMP, interrupciones, ASM switch/trap)
│   ├── src/bus/        # PCIe, enumeración de dispositivos
│   ├── src/storage/    # Controladores de almacenamiento (AHCI SATA)
│   ├── src/net/        # Tarjetas de red (Intel E1000, TCP/IP stack, Wi-Fi stub)
│   ├── src/mm/         # Gestión de memoria física (Frame), Heap, VM Space y DMA
│   ├── src/ffi_jail.rs # Aislamiento FFI para drivers externos
│   └── c_src/          # Drivers auxiliares en C (NPU hardware driver)
│
├── kernel_objects/     # Capa de Objetos de Kernel (Capacidades)
│   └── src/            # Process, Thread, VMO (Virtual Memory Object), Channel, Handle, Executor
│
├── linux_abi/          # Personalidad POSIX / Linux ABI Emulation
│   └── src/            # Syscall Dispatcher, VFS, TarFS, DiskFS, ELF Loader
│
├── kernel_main/        # Orquestación de Entrada del Kernel
│   └── src/main.rs     # Entry point desde Limine, inicialización de subsistemas y spawn Ring3
│
├── userland/           # Binarios de usuario en Rust (#![no_std])
├── c_std_userland/     # Binarios de usuario en C11 / POSIX (fipsos_sh, kilo_editor, pkg, httpd)
└── cpp_userland/       # Entorno Gráfico Ring 3 en C++20 (desktop_ui.cpp, avatar_cpp.cpp)
```

---

## 3. Reglas Inquebrantables de Arquitectura (Non-Negotiable)

1. **Entorno Estricto `#![no_std]`:**
   - Todo el workspace del kernel compila estrictamente **sin la librería estándar de Rust**.
   - Prohibido importar `std`. Únicamente se permite `core` y `alloc` (una vez inicializado el heap del kernel).
   - Flag `#![no_std]` y `#![no_main]` en cada crate de kernel.

2. **La Ley del Framekernel (Seguridad de Memoria):**
   - **`ostd` es el ÚNICO crate** donde se permite `unsafe`, ensamblador inline (`core::arch::asm!`), archivos ensamblador (`.s`) y puentes FFI (`ffi_jail`).
   - **`kernel_objects`**, **`linux_abi`** y **`kernel_main`** **DEBEN** tener en la primera línea de su `lib.rs` o `main.rs`:
     ```rust
     #![forbid(unsafe_code)]
     ```
   - Si un objeto o syscall necesita manipular hardware, registros o punteros físicos crudos, **debe abstraerse a través de una API segura expuesta por `ostd`**.

3. **Concurrencia Híbrida:**
   - **Espacio de Usuario (Ring 3):** Hilos con cambio de contexto cooperativo/preemptivo mediante `TrapFrame` en ASM.
   - **Espacio de Kernel (Ring 0):** Tareas asíncronas no bloqueantes basadas en `async/await` (Futures cooperativos sin stack).

4. **Integridad del Código (Cero Placeholders Destructivos):**
   - Nunca usar `todo!()`, `unimplemented!()` o valores dummy que silencien errores de tipo o lógica en rutas críticas de arranque o ejecución.
   - Todo cambio de código debe compilar limpiamente sin advertencias críticas ni roturas del ABI.

---

## 4. Comandos de Construcción, Ejecución y Pruebas

- **Compilar Kernel (Workspace de Rust):**
  ```powershell
  cargo build --workspace --target x86_64-unknown-none
  ```
- **Compilar Todo (Kernel + C Userland + C++ Desktop):**
  ```bash
  make all
  ```
- **Generar Imagen ISO:**
  ```bash
  make iso
  ```
- **Ejecutar en QEMU:**
  ```bash
  make run
  ```
  *(Parámetros QEMU: `-M q35 -cpu max -smp 2 -m 512M -drive id=satadisk,... -device ide-hd,... -device e1000e,...`)*
- **Pruebas Automatizadas (Python):**
  - Scripts en la raíz: `test_v22_consolidation.py`, `test_v23_consolidation.py`, `test_v31_dhcp_ansi_remote.py`, etc.
  - Se ejecutan con Python validando salida serial QMP/UART y captura de pantalla VGA.

---

## 5. Convenciones de Control de Versiones

- **Commits:** Seguir Conventional Commits (`feat:`, `fix:`, `refactor:`, `docs:`, `test:`).
- Siempre verificar `git status` y no commitear artefactos temporales de build (`target/`, `build/`, `.iso`, `.img`, logs de QEMU).

