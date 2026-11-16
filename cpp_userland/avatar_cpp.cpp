#include "fipsos_abi.hpp"
#include "avatar_render.hpp"

namespace {

size_t append_str(char* dst, size_t pos, const char* lit) noexcept {
    while (*lit != '\0' && pos < 184) {
        dst[pos++] = *lit++;
    }
    return pos;
}

size_t append_u32(char* dst, size_t pos, uint32_t val) noexcept {
    char tmp[12];
    size_t n = 0;
    if (val == 0) {
        tmp[n++] = '0';
    } else {
        while (val > 0 && n < sizeof(tmp)) {
            tmp[n++] = static_cast<char>('0' + (val % 10U));
            val /= 10U;
        }
    }
    while (n > 0 && pos < 184) {
        dst[pos++] = tmp[--n];
    }
    return pos;
}

size_t append_fp32(char* dst, size_t pos, float val) noexcept {
    if (val < 0.0f) {
        if (pos < 180) {
            dst[pos++] = '-';
        }
        val = -val;
    }
    const uint32_t scaled = static_cast<uint32_t>(val * 1000.0f + 0.5f);
    const uint32_t whole  = scaled / 1000U;
    const uint32_t frac   = scaled % 1000U;
    pos = append_u32(dst, pos, whole);
    if (pos + 4 < 186) {
        dst[pos++] = '.';
        dst[pos++] = static_cast<char>('0' + ((frac / 100U) % 10U));
        dst[pos++] = static_cast<char>('0' + ((frac / 10U) % 10U));
        dst[pos++] = static_cast<char>('0' + (frac % 10U));
    }
    return pos;
}

void sync_npu_fp32_weights(const volatile uint8_t* accel_vmo, AvatarHeapState* state) noexcept {
    if (accel_vmo == nullptr) {
        return;
    }
    const auto* dma_floats = reinterpret_cast<const volatile float*>(accel_vmo + 128);
    for (size_t i = 0; i < 16; ++i) {
        state->fp32_weights[i] = dma_floats[i];
    }
}

} // namespace

extern "C" void avatar_main() noexcept {
    using namespace fipsos;

    print_literal(
        "[avatar_cpp.elf] Motor 3D Phong C++20 v1.3 (FP32 SSE2 + Mouse0 + Net eth0 e1000e) en Anillo 3!\n"
    );

    const int64_t pid = sys_getpid();

    const uint64_t heap_base = sys_brk(0);
    const uint64_t heap_end  = sys_brk(heap_base + 4096);
    if (heap_end <= heap_base) {
        sys_exit(1);
    }

    auto* state = reinterpret_cast<AvatarHeapState*>(heap_base);
    state->yaw_rad              = 0.35f;
    state->pitch_rad            = -0.22f;
    state->radius_px            = 30.0f;
    state->normal_norm_sample   = 0.0f;
    state->phong_peak_sample    = 0.0f;
    state->npu_energy_milli     = 0;
    state->palette_mode         = 0;
    state->frames_rendered      = 0;
    state->npu_inferences       = 0;
    state->mouse_x              = 640;
    state->mouse_y              = 400;
    state->mouse_buttons        = 0;
    state->mouse_seq            = 0;
    state->mouse_events_handled = 0;
    state->net_tx_pkts          = 0;
    state->net_rx_pkts          = 0;
    state->net_rtt_us           = 0;
    state->net_pings            = 0;
    state->pid                  = pid;

    const float raw_lx  = -0.42f;
    const float raw_ly  = -0.55f;
    const float raw_lz  = 0.72f;
    const float inv_len = 1.0f / sqrtf_sse2(raw_lx * raw_lx + raw_ly * raw_ly + raw_lz * raw_lz);
    state->light_dir[0] = raw_lx * inv_len;
    state->light_dir[1] = raw_ly * inv_len;
    state->light_dir[2] = raw_lz * inv_len;

    for (size_t i = 0; i < 16; ++i) {
        state->fp32_weights[i] = 0.50f;
    }

    // Open `/dev/input/mouse0` and synchronize initial PS/2 pointer coordinates
    const int64_t mouse_fd = sys_open("/dev/input/mouse0", O_RDWR);
    if (mouse_fd >= 0) {
        const int64_t raw_mouse = sys_ioctl(static_cast<uint64_t>(mouse_fd), MOUSE_IOCTL_POLL_STATE, 0);
        if (raw_mouse >= 0) {
            const MousePacketState ms = unpack_mouse_state(static_cast<uint64_t>(raw_mouse));
            state->mouse_x       = ms.x;
            state->mouse_y       = ms.y;
            state->mouse_buttons = ms.buttons;
            state->mouse_seq     = ms.event_seq;
        }
    }

    // Open `/dev/net/eth0` (PCIe Intel e1000e) and probe initial gateway telemetry (`10.0.2.2`)
    const int64_t net_fd = sys_open("/dev/net/eth0", O_RDWR);
    if (net_fd >= 0) {
        const int64_t raw_net = sys_ioctl(static_cast<uint64_t>(net_fd), NET_IOCTL_PING_GATEWAY, 0);
        if (raw_net >= 0) {
            const NetPingTelemetry nt = unpack_net_telemetry(static_cast<uint64_t>(raw_net));
            state->net_tx_pkts = nt.tx_packets;
            state->net_rx_pkts = nt.rx_packets;
            state->net_rtt_us  = nt.rtt_us;
            state->net_pings   = 1U;
        }
    }

    const int64_t model_fd = sys_open("/models/avatar_v1.bin", O_RDONLY);
    const int64_t accel_fd = sys_open("/dev/accel/accel0", O_RDWR);
    const volatile uint8_t* accel_vmo = nullptr;

    if (accel_fd >= 0) {
        const int64_t mapped_npu = sys_mmap(
            0, 4096, PROT_READ, MAP_SHARED, static_cast<uint64_t>(accel_fd), 0
        );
        if (mapped_npu > 0) {
            accel_vmo = reinterpret_cast<const volatile uint8_t*>(mapped_npu);
        }
    }

    if (model_fd >= 0 && accel_fd >= 0) {
        const int64_t energy_milli = sys_ioctl(
            static_cast<uint64_t>(accel_fd),
            ACCEL_IOCTL_RUN_INFERENCE,
            static_cast<uint64_t>(model_fd)
        );
        if (energy_milli >= 0) {
            state->npu_energy_milli = static_cast<uint32_t>(energy_milli);
            state->npu_inferences += 1U;
            sync_npu_fp32_weights(accel_vmo, state);
        }
    }

    const int64_t fb_fd = sys_open("/dev/fb0", O_RDWR);
    volatile uint32_t* fb_vmo = nullptr;
    constexpr uint64_t kPackedDims =
        (static_cast<uint64_t>(AvatarEngine3D::kWidth) << 16) |
        static_cast<uint64_t>(AvatarEngine3D::kHeight);

    if (fb_fd >= 0) {
        constexpr size_t kByteLen = AvatarEngine3D::kWidth * AvatarEngine3D::kHeight * 4;
        const int64_t mapped = sys_mmap(
            0, kByteLen, PROT_READ | PROT_WRITE, MAP_SHARED, static_cast<uint64_t>(fb_fd), 0
        );
        if (mapped > 0) {
            fb_vmo = reinterpret_cast<volatile uint32_t*>(mapped);
        }
    }

    auto present_current_frame = [&]() noexcept {
        if (fb_vmo != nullptr && fb_fd >= 0) {
            AvatarEngine3D::render_frame(fb_vmo, state);
            sys_ioctl(static_cast<uint64_t>(fb_fd), FB_IOCTL_PRESENT_RECT, kPackedDims);
        }
    };

    present_current_frame();
    {
        const float l_norm = sqrtf_sse2(
            state->light_dir[0] * state->light_dir[0] +
            state->light_dir[1] * state->light_dir[1] +
            state->light_dir[2] * state->light_dir[2]
        );
        size_t vlen = 0;
        vlen = append_str(state->fmt_buf, vlen, "[avatar_cpp::sse2] FP32 SSE2 + Mouse0: |N|=");
        vlen = append_fp32(state->fmt_buf, vlen, state->normal_norm_sample);
        vlen = append_str(state->fmt_buf, vlen, " | |L|=");
        vlen = append_fp32(state->fmt_buf, vlen, l_norm);
        vlen = append_str(state->fmt_buf, vlen, " | Cursor=(");
        vlen = append_u32(state->fmt_buf, vlen, state->mouse_x);
        vlen = append_str(state->fmt_buf, vlen, ",");
        vlen = append_u32(state->fmt_buf, vlen, state->mouse_y);
        vlen = append_str(state->fmt_buf, vlen, ") | NPU_FP32[0]=");
        vlen = append_fp32(state->fmt_buf, vlen, state->fp32_weights[0]);
        vlen = append_str(state->fmt_buf, vlen, "\n");
        sys_write(FD_STDOUT, state->fmt_buf, vlen);
    }

    print_literal(
        "[avatar_cpp::loop] Activo: Raton PS/2 | [W/A/S/D] Rotar | [M] Step Raton | [P] Ping eth0 | [N] NPU | [Q] Salir\n"
    );

    bool interactive_engaged = false;
    uint32_t idle_polls = 0;
    const uint32_t max_initial_polls = 120;
    const uint32_t max_engaged_polls = 240;

    while (fb_fd >= 0) {
        bool handled_input = false;

        // 1. Poll real-time PS/2 pointer events from `/dev/input/mouse0`
        if (mouse_fd >= 0) {
            const int64_t raw_mouse = sys_ioctl(
                static_cast<uint64_t>(mouse_fd),
                MOUSE_IOCTL_POLL_STATE,
                0
            );
            if (raw_mouse >= 0) {
                const MousePacketState ms = unpack_mouse_state(static_cast<uint64_t>(raw_mouse));
                if ((ms.buttons & MOUSE_BTN_RIGHT) != 0U) {
                    print_literal("[avatar_cpp::input] Clic Derecho -> Retornando al Escritorio...\n");
                    break;
                }
                if (ms.event_seq != state->mouse_seq) {
                    if ((ms.buttons & MOUSE_BTN_LEFT) != 0U) {
                        interactive_engaged = true;
                        idle_polls = 0;
                    }
                    handled_input = true;
                    steer_scene_from_mouse(state, ms);
                    state->mouse_events_handled += 1U;
                    present_current_frame();

                    if (state->mouse_events_handled <= 4U) {
                        size_t mlen = 0;
                        mlen = append_str(state->fmt_buf, mlen, "[avatar_cpp::mouse] Evento PS/2 #");
                        mlen = append_u32(state->fmt_buf, mlen, state->mouse_events_handled);
                        mlen = append_str(state->fmt_buf, mlen, " -> Cursor=(");
                        mlen = append_u32(state->fmt_buf, mlen, state->mouse_x);
                        mlen = append_str(state->fmt_buf, mlen, ",");
                        mlen = append_u32(state->fmt_buf, mlen, state->mouse_y);
                        mlen = append_str(state->fmt_buf, mlen, ") Btn=");
                        mlen = append_u32(state->fmt_buf, mlen, state->mouse_buttons);
                        mlen = append_str(state->fmt_buf, mlen, " | Lz=");
                        mlen = append_fp32(state->fmt_buf, mlen, state->light_dir[2]);
                        mlen = append_str(state->fmt_buf, mlen, "\n");
                        sys_write(FD_STDOUT, state->fmt_buf, mlen);
                    }
                }
            }
        }

        // 2. Poll real-time non-blocking keyboard commands (`W/A/S/D/M/N/Q`)
        const int64_t key = sys_ioctl(static_cast<uint64_t>(fb_fd), FB_IOCTL_POLL_KEY, 0);
        if (key > 0) {
            handled_input = true;
            const char ch = static_cast<char>(key & 0xFF);
            if (ch == 'a' || ch == 'A') {
                interactive_engaged = true;
                idle_polls = 0;
                state->yaw_rad = wrap_pi(state->yaw_rad - 0.28f);
                present_current_frame();
                print_literal("[avatar_cpp::input] Tecla 'A' -> Yaw FP32 (-0.28 rad) en /dev/fb0\n");
            } else if (ch == 'd' || ch == 'D') {
                interactive_engaged = true;
                idle_polls = 0;
                state->yaw_rad = wrap_pi(state->yaw_rad + 0.28f);
                present_current_frame();
                print_literal("[avatar_cpp::input] Tecla 'D' -> Yaw FP32 (+0.28 rad) en /dev/fb0\n");
            } else if (ch == 'w' || ch == 'W') {
                interactive_engaged = true;
                idle_polls = 0;
                state->pitch_rad = wrap_pi(state->pitch_rad - 0.22f);
                if (state->radius_px < 35.0f) {
                    state->radius_px += 1.5f;
                }
                present_current_frame();
                print_literal("[avatar_cpp::input] Tecla 'W' -> Pitch/Zoom FP32 (-0.22 rad) en /dev/fb0\n");
            } else if (ch == 's' || ch == 'S') {
                interactive_engaged = true;
                idle_polls = 0;
                state->pitch_rad = wrap_pi(state->pitch_rad + 0.22f);
                if (state->radius_px > 22.0f) {
                    state->radius_px -= 1.5f;
                }
                present_current_frame();
                print_literal("[avatar_cpp::input] Tecla 'S' -> Pitch/Zoom FP32 (+0.22 rad) en /dev/fb0\n");
            } else if (ch == 'm' || ch == 'M') {
                interactive_engaged = true;
                idle_polls = 0;
                if (mouse_fd >= 0) {
                    const uint64_t step_arg = pack_synthetic_mouse_step(96, -56, MOUSE_BTN_LEFT);
                    const int64_t raw_after = sys_ioctl(
                        static_cast<uint64_t>(mouse_fd),
                        MOUSE_IOCTL_STEP_SYNTHETIC,
                        step_arg
                    );
                    if (raw_after >= 0) {
                        const MousePacketState ms = unpack_mouse_state(static_cast<uint64_t>(raw_after));
                        steer_scene_from_mouse(state, ms);
                        state->mouse_events_handled += 1U;
                    }
                }
                present_current_frame();
                print_literal(
                    "[avatar_cpp::mouse] Tecla 'M' -> Step sintetico + Clic Izq aplicado a light_dir/esfera FP32!\n"
                );
            } else if (ch == 'p' || ch == 'P') {
                interactive_engaged = true;
                idle_polls = 0;
                if (net_fd >= 0) {
                    const int64_t raw_net = sys_ioctl(
                        static_cast<uint64_t>(net_fd),
                        NET_IOCTL_PING_GATEWAY,
                        1
                    );
                    if (raw_net >= 0) {
                        const NetPingTelemetry nt = unpack_net_telemetry(static_cast<uint64_t>(raw_net));
                        state->net_tx_pkts = nt.tx_packets;
                        state->net_rx_pkts = nt.rx_packets;
                        state->net_rtt_us  = nt.rtt_us;
                        state->net_pings  += 1U;
                    }
                }
                present_current_frame();
                size_t nlen = 0;
                nlen = append_str(state->fmt_buf, nlen, "[avatar_cpp::net] Tecla 'P' -> Ping eth0 10.0.2.2 OK | TX=");
                nlen = append_u32(state->fmt_buf, nlen, state->net_tx_pkts);
                nlen = append_str(state->fmt_buf, nlen, " RX=");
                nlen = append_u32(state->fmt_buf, nlen, state->net_rx_pkts);
                nlen = append_str(state->fmt_buf, nlen, " RTT=");
                nlen = append_u32(state->fmt_buf, nlen, state->net_rtt_us);
                nlen = append_str(state->fmt_buf, nlen, "us (Halo 3D sincronizado)\n");
                sys_write(FD_STDOUT, state->fmt_buf, nlen);
            } else if (ch == 'n' || ch == 'N') {
                interactive_engaged = true;
                idle_polls = 0;
                if (model_fd >= 0 && accel_fd >= 0) {
                    const int64_t energy_milli = sys_ioctl(
                        static_cast<uint64_t>(accel_fd),
                        ACCEL_IOCTL_RUN_INFERENCE,
                        static_cast<uint64_t>(model_fd)
                    );
                    if (energy_milli >= 0) {
                        state->npu_energy_milli = static_cast<uint32_t>(energy_milli);
                        sync_npu_fp32_weights(accel_vmo, state);
                    }
                }
                state->palette_mode = (state->palette_mode + 1U) & 3U;
                state->npu_inferences += 1U;
                present_current_frame();
                print_literal(
                    "[avatar_cpp::npu] Tecla 'N' -> Inferencia C11 FP32 GEMM + Phong 3D actualizado!\n"
                );
            } else if (ch == 'q' || ch == 'Q' || ch == 0x1B) {
                print_literal(
                    "[avatar_cpp::input] Tecla 'Q' -> Cerrando Event Loop Phong 3D + Mouse0 y liberando descriptores...\n"
                );
                break;
            }
        }

        if (!handled_input) {
            for (uint32_t spin = 0; spin < 25000U; ++spin) {
                asm volatile("pause");
            }
            if ((idle_polls & 31U) == 0U) {
                sys_sched_yield();
            }
            idle_polls += 1;
            const uint32_t limit = interactive_engaged ? max_engaged_polls : max_initial_polls;
            if (idle_polls >= limit) {
                break;
            }
        }
    }

    if (net_fd >= 0) {
        sys_close(static_cast<uint64_t>(net_fd));
    }
    if (mouse_fd >= 0) {
        sys_close(static_cast<uint64_t>(mouse_fd));
    }
    if (fb_fd >= 0) {
        sys_close(static_cast<uint64_t>(fb_fd));
    }
    if (accel_fd >= 0) {
        sys_close(static_cast<uint64_t>(accel_fd));
    }
    if (model_fd >= 0) {
        sys_close(static_cast<uint64_t>(model_fd));
    }

    size_t len = 0;
    len = append_str(state->fmt_buf, len, "[avatar_cpp.elf] Resumen v1.3 FP32+Mouse+Net: Frames=");
    len = append_u32(state->fmt_buf, len, state->frames_rendered);
    len = append_str(state->fmt_buf, len, " | Mouse=(");
    len = append_u32(state->fmt_buf, len, state->mouse_x);
    len = append_str(state->fmt_buf, len, ",");
    len = append_u32(state->fmt_buf, len, state->mouse_y);
    len = append_str(state->fmt_buf, len, ") | Net(TX=");
    len = append_u32(state->fmt_buf, len, state->net_tx_pkts);
    len = append_str(state->fmt_buf, len, ",RX=");
    len = append_u32(state->fmt_buf, len, state->net_rx_pkts);
    len = append_str(state->fmt_buf, len, ") | |N|=");
    len = append_fp32(state->fmt_buf, len, state->normal_norm_sample);
    len = append_str(state->fmt_buf, len, " OK!\n");
    sys_write(FD_STDOUT, state->fmt_buf, len);

    sys_exit(0);
}

extern "C" [[noreturn]] __attribute__((naked, section(".text._start"))) void _start() noexcept {
    asm volatile(
        "andq $-16, %%rsp\n\t"
        "call avatar_main\n\t"
        "xorl %%edi, %%edi\n\t"
        "movl $60, %%eax\n\t"
        "syscall\n\t"
        ::: "memory"
    );
}
