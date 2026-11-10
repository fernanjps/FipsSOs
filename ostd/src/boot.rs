use limine::request::{
    ExecutableFileRequest, FramebufferRequest, HhdmRequest, MemoryMapRequest, ModuleRequest,
    MpRequest, RequestsEndMarker, RequestsStartMarker,
};
use limine::BaseRevision;

/// Marks the start of the Limine boot request section.
#[used]
#[link_section = ".limine_requests_start"]
static REQUESTS_START_MARKER: RequestsStartMarker = RequestsStartMarker::new();

/// Requests Limine protocol base revision 3 (latest supported by `limine` 0.5).
#[used]
#[link_section = ".limine_requests"]
static BASE_REVISION: BaseRevision = BaseRevision::new();

/// Requests a linear framebuffer from the Limine bootloader.
#[used]
#[link_section = ".limine_requests"]
static FRAMEBUFFER_REQUEST: FramebufferRequest = FramebufferRequest::new();

/// Requests the Higher-Half Direct Map (HHDM) virtual offset from Limine.
#[used]
#[link_section = ".limine_requests"]
static HHDM_REQUEST: HhdmRequest = HhdmRequest::new();

/// Requests the physical memory map from the Limine bootloader.
#[used]
#[link_section = ".limine_requests"]
static MEMORY_MAP_REQUEST: MemoryMapRequest = MemoryMapRequest::new();

/// Requests boot modules (such as `initramfs.tar`) from the Limine bootloader.
#[used]
#[link_section = ".limine_requests"]
static MODULE_REQUEST: ModuleRequest = ModuleRequest::new();

/// Requests Symmetric Multi-Processing (SMP / MP) topology and AP startup from Limine.
#[used]
#[link_section = ".limine_requests"]
static MP_REQUEST: MpRequest = MpRequest::new();

/// Requests the kernel executable file and cmdline from the Limine bootloader.
#[used]
#[link_section = ".limine_requests"]
static EXECUTABLE_FILE_REQUEST: ExecutableFileRequest = ExecutableFileRequest::new();

/// Marks the end of the Limine boot request section.
#[used]
#[link_section = ".limine_requests_end"]
static REQUESTS_END_MARKER: RequestsEndMarker = RequestsEndMarker::new();

/// Base background color painted on boot (FipSOs Deep Cyber Blue: `#1B2A4A`).
pub const BOOT_FB_COLOR_RGB: u32 = 0x001B_2A4A;

extern "Rust" {
    static __fipsos_kernel_main_entry: fn();
}

/// Low-level ELF entry point invoked directly by the Limine bootloader.
///
/// This symbol resides in `ostd` so that `kernel_main` can remain 100% safe
/// under `#![forbid(unsafe_code)]`.
#[no_mangle]
pub extern "C" fn _start() -> ! {
    crate::arch::serial::init();

    // SAFETY: `__fipsos_kernel_main_entry` is populated at link time by
    // `ostd::kernel_main!` and verified to be exactly 8 bytes by `linker_x86_64.ld`.
    let main_fn: fn() = unsafe { core::ptr::read_volatile(&raw const __fipsos_kernel_main_entry) };
    main_fn();

    crate::halt_loop()
}

/// Verifies the Limine bootloader handoff, paints a base color across the
/// hardware linear framebuffer, and initializes `ostd::mm` (HHDM, FrameAllocator, and Heap).
pub fn init() {
    crate::arch::serial::init();

    assert!(
        BASE_REVISION.is_supported(),
        "Limine Base Revision no soportada por el bootloader."
    );

    let fb_response = FRAMEBUFFER_REQUEST
        .get_response()
        .expect("Limine no devolvio respuesta para FramebufferRequest.");

    let framebuffer = fb_response
        .framebuffers()
        .next()
        .expect("Limine no proporciono ningun Framebuffer activo.");

    let width = framebuffer.width() as usize;
    let height = framebuffer.height() as usize;
    let pitch = framebuffer.pitch() as usize;
    let bpp = framebuffer.bpp() as usize;
    let fb_ptr = framebuffer.addr();

    if bpp == 32 {
        let pixels_per_row = pitch / 4;
        for y in 0..height {
            let row_offset = y * pixels_per_row;
            for x in 0..width {
                // SAFETY: `fb_ptr` points to the Limine-mapped linear framebuffer in
                // Higher-Half HHDM memory, and `(y * pitch) + (x * 4)` is within bounds
                // of `height * pitch`.
                unsafe {
                    let pixel_ptr = fb_ptr.cast::<u32>().add(row_offset + x);
                    core::ptr::write_volatile(pixel_ptr, BOOT_FB_COLOR_RGB);
                }
            }
        }
    }

    crate::fb_console::init(fb_ptr, width, height, pitch, bpp);

    crate::early_println!(
        "[ostd::boot] Limine Higher-Half OK | Framebuffer {}x{} ({} bpp, pitch={}) pintado con #1B2A4A",
        width,
        height,
        bpp,
        pitch
    );
    crate::fb_println!(
        "[ostd::boot] Limine Higher-Half OK | Framebuffer {}x{} ({} bpp, pitch={}) Consola 8x8 activa",
        width,
        height,
        bpp,
        pitch
    );

    let hhdm_response = HHDM_REQUEST
        .get_response()
        .expect("Limine no devolvio respuesta para HhdmRequest.");

    let memmap_response = MEMORY_MAP_REQUEST
        .get_response()
        .expect("Limine no devolvio respuesta para MemoryMapRequest.");

    crate::mm::init(hhdm_response.offset(), memmap_response.entries());
    crate::fb_console::init_ram_backbuffer();
    #[cfg(target_arch = "x86_64")]
    crate::arch::fpu::init();
    crate::arch::init();
    crate::net::init();
    crate::storage::init();
    crate::bus::xhci::init();

    if let Some(initramfs) = initramfs_slice() {
        crate::early_println!(
            "[ostd::boot] Modulo Limine Initramfs detectado ({} bytes)",
            initramfs.len()
        );
        crate::fb_println!(
            "[ostd::boot] Modulo Limine Initramfs detectado ({} bytes)",
            initramfs.len()
        );
    }
}

/// Returns a safe byte slice of the first boot module (`initramfs.tar`) loaded by Limine,
/// or `None` if no module was provided by the bootloader.
pub fn initramfs_slice() -> Option<&'static [u8]> {
    let response = MODULE_REQUEST.get_response()?;
    let module = response.modules().first()?;
    let ptr = module.addr();
    let len = module.size() as usize;
    if ptr.is_null() || len == 0 {
        return None;
    }
    // SAFETY: Limine maps module memory into Higher-Half HHDM for the lifetime of the kernel.
    Some(unsafe { core::slice::from_raw_parts(ptr, len) })
}

/// Returns the Limine MP / SMP response if present.
pub fn smp_response() -> Option<&'static limine::response::MpResponse> {
    MP_REQUEST.get_response()
}

/// Returns the Limine executable file response, if available.
pub fn executable_file_response() -> Option<&'static limine::response::ExecutableFileResponse> {
    EXECUTABLE_FILE_REQUEST.get_response()
}

/// Returns the kernel boot command line string if passed by the bootloader.
pub fn kernel_cmdline() -> Option<&'static str> {
    let resp = executable_file_response()?;
    resp.file().string().to_str().ok()
}
