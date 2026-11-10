# FipSOs - Reglas Inquebrantables de Arquitectura (Framekernel)

1. **Identidad:** El sistema operativo se llama **FipSOs**. Es un Framekernel multi-arquitectura (`x86_64` y `aarch64`) basado en capacidades (estilo Zircon) con personalidad Linux/POSIX desacoplada y soporte nativo Zero-Copy para GPU (DRM) y NPU (`/dev/accel`).
2. **Entorno `#![no_std]`:** Todo el workspace compila sin la librería estándar de Rust (`#![no_std]` y `#![no_main]` donde aplique). Prohibido importar `std`. Solo se permite `core` y `alloc` (una vez inicializado el heap).
3. **Ley del Framekernel (Seguridad de Memoria):**
   - `ostd`: ÚNICO crate donde se permite `unsafe`, ensamblador (`core::arch::asm!` / archivos `.s`) y puente FFI (`ffi_jail`).
   - `kernel_objects`, `linux_abi`, `kernel_main`: DEBEN tener `#![forbid(unsafe_code)]` en la línea 1 de su `lib.rs` o `main.rs`. Si necesitas tocar hardware o punteros crudos, debes exponer una API segura en `ostd`.
4. **Bootloader:** Se utiliza el protocolo **Limine** (Higher-Half Kernel mapeado en `0xFFFFFFFF80000000`).
5. **Concurrencia:** El núcleo usa un modelo híbrido: hilos de usuario con contexto en ASM (`TrapFrame`) y tareas internas del kernel basadas en `async/await` (Futures sin stack).
6. **Regla de Oro para el Agente:** NUNCA avances a la siguiente fase sin que la fase actual compile con `cargo build` sin errores y pase la prueba en QEMU. No uses código "placeholder" que oculte errores de tipos.
