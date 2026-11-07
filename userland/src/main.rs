#![forbid(unsafe_code)]
#![no_std]
#![no_main]

use libfipsos::{
    print, render_zero_copy_rgb32_widget, sys_exit, sys_getpid, sys_ioctl, sys_mmap, sys_open,
    sys_sched_yield, ACCEL_IOCTL_RUN_INFERENCE, CSTR_PATH_ACCEL0, CSTR_PATH_FB0,
    CSTR_PATH_MODEL_AVATAR, FB_IOCTL_PRESENT_RECT, FB_WIDGET_H, FB_WIDGET_W, MAP_SHARED, O_RDONLY,
    O_RDWR, PROT_READ, PROT_WRITE,
};

ostd::user_main!(npu_infer_main);

fn npu_infer_main() -> i32 {
    // 1. Announce native Rust ELF64 execution in Ring 3 at RIP = 0x0050_0000
    print(
        "[npu_infer.elf] Binario ELF64 compilado en Rust nativo ejecutandose en Anillo 3 (RIP=0x500000)!\n",
    );

    // 2. Query our Process KOID via sys_getpid (39) and cooperatively yield via sys_sched_yield (24)
    let pid = sys_getpid();
    if pid < 0 {
        sys_exit(10);
    }
    let _ = sys_sched_yield();

    // 3. Open the AI model VMO (/models/avatar_v1.bin) and NPU accelerator (/dev/accel/accel0)
    let model_fd = sys_open(CSTR_PATH_MODEL_AVATAR, O_RDONLY);
    if model_fd < 0 {
        print("[npu_infer.elf] Error al abrir /models/avatar_v1.bin\n");
        sys_exit(1);
    }

    let accel_fd = sys_open(CSTR_PATH_ACCEL0, O_RDWR);
    if accel_fd < 0 {
        print("[npu_infer.elf] Error al abrir /dev/accel/accel0\n");
        sys_exit(2);
    }

    // 4. Trigger Zero-Copy NPU inference binding model_fd -> accel_fd
    let score = sys_ioctl(
        accel_fd as u64,
        ACCEL_IOCTL_RUN_INFERENCE,
        model_fd as u64,
    );
    if score < 0 {
        print("[npu_infer.elf] Error en sys_ioctl(ACCEL_IOCTL_RUN_INFERENCE)\n");
        sys_exit(3);
    }

    // 5. Yield CPU quantum after NPU dispatch so npu-telemetryd collects live telemetry
    let _ = sys_sched_yield();

    // 6. Open and mmap /dev/fb0 Zero-Copy VMO to render the Rust ELF64 AI inference overlay
    let fb_fd = sys_open(CSTR_PATH_FB0, O_RDWR);
    if fb_fd >= 0 {
        let byte_len = FB_WIDGET_W * FB_WIDGET_H * 4;
        let mapped_vaddr = sys_mmap(
            0,
            byte_len,
            PROT_READ | PROT_WRITE,
            MAP_SHARED,
            fb_fd as u64,
            0,
        );
        if mapped_vaddr > 0 {
            let color_seed = (score as u32) ^ (pid as u32) ^ 0x00FF_7A20;
            let _ = render_zero_copy_rgb32_widget(
                mapped_vaddr as u64,
                FB_WIDGET_W,
                FB_WIDGET_H,
                color_seed,
                0x0001_0D07,
            );
            let packed_dims = ((FB_WIDGET_W as u64) << 16) | (FB_WIDGET_H as u64);
            let _ = sys_ioctl(fb_fd as u64, FB_IOCTL_PRESENT_RECT, packed_dims);
        }
    }

    print(
        "[npu_infer.elf] Pipeline Zero-Copy /models/avatar_v1.bin -> /dev/accel/accel0 + /dev/fb0 completado desde Rust!\n",
    );
    sys_exit(0)
}
