#!/usr/bin/env python3
"""
FipSOs v6.1 Live Kernel Telemetry, Memory Reclamation & GUI Monitor E2E Test:
  1. Boots `build/fipsos-x86_64.iso` under VirtualBox-like QEMU profile
     (-M q35 -cpu EPYC,+hypervisor -smp 2 -m 2048M + 2 GiB SATA AHCI + xHCI HID + e1000e).
  2. Verifies `FipSOs v6.1` boot banner and `uname -a -> 6.1.0-FipSOs`.
  3. Measures baseline free 4K frames via `/sys/fipsos/hwmon`, runs repeated child ELFs
     (/bin/npu_infer.elf, /bin/c_std_app.elf), and verifies frame reclamation.
  4. Launches `gui` (`desktop_ui.elf` v6.1), captures Clean Desktop, opens Live Kernel Monitor
     (`wins[0]`) from Dock Icon #1, verifies SMP cores/RAM/process table live rendering & tick progression.
  5. Clicks `"Probar RAM R/W"` (HHDM 256x4K volatile write/verify probe), clicks `"Lanzar Proceso"`
     4x (alternating `/bin/npu_infer.elf` & `/bin/c_std_app.elf`), clicks `"Ciclar Perfil"` &
     `"Refrescar Vivo"`, verifying live UI pixel updates and zero process/slot/memory exhaustion.
  6. Exits GUI cleanly (`q`) back to `fipsos:/#` and verifies post-GUI free 4K frames via `/sys/fipsos/hwmon`.
"""

import os
import re
import subprocess
import sys
import time

import test_v51_layer3_suite as base

REPO_DIR = base.REPO_DIR
ISO_PATH = base.ISO_PATH
USB_IMG_PATH = os.path.join(REPO_DIR, "build", "fipsos-baremetal-usb.img")
DISK_PATH = os.path.join(REPO_DIR, "build", "test_v61_sata.img")
SERIAL_LOG = os.path.join(REPO_DIR, "build", "test_v61_serial.log")
QMP_PORT = 4493

base.DISK_PATH = DISK_PATH
base.SERIAL_LOG = SERIAL_LOG
base.QMP_PORT = QMP_PORT


def create_2g_disk(path: str) -> None:
    os.makedirs(os.path.dirname(path), exist_ok=True)
    if os.path.exists(path):
        os.remove(path)
    with open(path, "wb") as f:
        f.truncate(2048 * 1024 * 1024)


def load_ppm_rgb(ppm_path: str) -> tuple[int, int, bytes]:
    with open(ppm_path, "rb") as f:
        tokens = []
        while len(tokens) < 4:
            line = f.readline()
            if not line.startswith(b"#"):
                tokens.extend(line.split())
        w = int(tokens[1])
        h = int(tokens[2])
        data = f.read(w * h * 3)
    return w, h, data


def region_signature(rgb: bytes, w: int, x0: int, y0: int, rw: int, rh: int) -> tuple[int, int, int, int]:
    r_sum = 0
    g_sum = 0
    b_sum = 0
    xor_hash = 0
    cnt = 0
    for y in range(y0, y0 + rh, 2):
        for x in range(x0, x0 + rw, 2):
            idx = (y * w + x) * 3
            r = rgb[idx]
            g = rgb[idx + 1]
            b = rgb[idx + 2]
            r_sum += r
            g_sum += g
            b_sum += b
            xor_hash = ((xor_hash * 131) ^ (r << 16) ^ (g << 8) ^ b) & 0xFFFFFFFF
            cnt += 1
    return (r_sum // max(1, cnt), g_sum // max(1, cnt), b_sum // max(1, cnt), xor_hash)


def send_shell_text(qmp: base.QmpClient, text: str) -> None:
    extra = {
        "\\": ["backslash"],
        "&": ["shift", "7"],
        "%": ["shift", "5"],
        "^": ["shift", "6"],
        "$": ["shift", "4"],
        "@": ["shift", "2"],
        "~": ["shift", "grave_accent"],
    }
    for ch in text:
        if ch in extra:
            res = qmp.cmd("send-key", {"keys": [{"type": "qcode", "data": k} for k in extra[ch]]})
            if "error" in res:
                raise RuntimeError(f"QMP error on {ch!r}: {res}")
            time.sleep(0.04)
        else:
            qmp.send_text(ch)


def query_hwmon_frames(qmp: base.QmpClient, tag: str) -> tuple[int, int, int, str]:
    time.sleep(0.32)
    pos = len(base.read_serial())
    qmp.send_text("cat /sys/fipsos/hwmon\n")
    ok, _ = base.wait_for_serial("Recuperacion de Fallos Anillo 3", timeout=10.0, offset=pos)
    tail = base.strip_ansi(base.read_serial()[pos:])
    assert ok, f"[{tag}] /sys/fipsos/hwmon timed out:\n{tail}"
    ok_p, _ = base.wait_for_serial("fipsos:/#", timeout=10.0, offset=pos)
    assert ok_p, f"[{tag}] Shell prompt missing after /sys/fipsos/hwmon:\n{base.strip_ansi(base.read_serial()[pos:])}"
    m = re.search(
        r"Memoria Fisica:\s+(\d+)\s+KiB Total\s+\|\s+(\d+)\s+KiB Usados\s+\((\d+)\s+frames\)\s+\|\s+(\d+)\s+KiB Libres\s+\((\d+)\s+frames\)",
        tail,
    )
    assert m is not None, f"[{tag}] Could not parse Memoria Fisica from hwmon:\n{tail}"
    used_fr = int(m.group(3))
    free_fr = int(m.group(5))
    c1_idle = 0
    m_cpu = re.search(r"Core 0:\s+(\d+)% uso \(idle:\s+(\d+)\s+ticks\)\s+\|\s+Core 1:\s+(\d+)% uso \(idle:\s+(\d+)\s+ticks\)", tail)
    if m_cpu:
        c1_idle = int(m_cpu.group(4))
    return used_fr, free_fr, c1_idle, tail


def main() -> int:
    print("=== FipSOs v6.1 Live Kernel Telemetry & Reclamation Verification ===")
    assert os.path.exists(ISO_PATH), f"Missing {ISO_PATH}"
    assert os.path.exists(USB_IMG_PATH), f"Missing {USB_IMG_PATH}"
    create_2g_disk(DISK_PATH)
    if os.path.exists(SERIAL_LOG):
        os.remove(SERIAL_LOG)

    qemu_cmd = [
        "qemu-system-x86_64",
        "-M", "q35",
        "-cpu", "EPYC,+hypervisor",
        "-smp", "2",
        "-m", "2048M",
        "-no-reboot",
        "-no-shutdown",
        "-serial", f"file:{SERIAL_LOG}",
        "-qmp", f"tcp:127.0.0.1:{QMP_PORT},server,nowait",
        "-drive", f"id=satadisk,file={DISK_PATH},format=raw,if=none",
        "-device", "ide-hd,drive=satadisk,bus=ide.0",
        "-device", "qemu-xhci,id=xhci0",
        "-device", "usb-kbd,bus=xhci0.0",
        "-device", "usb-mouse,bus=xhci0.0",
        "-netdev", "user,id=net0",
        "-device", "e1000e,netdev=net0",
        "-boot", "d",
        "-cdrom", ISO_PATH,
    ]
    proc = subprocess.Popen(qemu_cmd)
    qmp = None
    ppm_files: list[str] = []
    try:
        qmp = base.QmpClient(QMP_PORT)
        ok, _ = base.wait_for_serial("fipsos:/#", timeout=45.0)
        assert ok, f"Shell prompt not reached. Tail:\n{base.read_serial()[-1200:]}"

        boot_log = base.strip_ansi(base.read_serial())
        assert "FipSOs v6.1" in boot_log, "Missing FipSOs v6.1 boot banner"
        print("[PASS] Booted FipSOs v6.1 on Q35 + EPYC,+hypervisor + 2 cores + 2048 MiB")

        pos_u = len(base.read_serial())
        qmp.send_text("uname -a\n")
        ok_u, _ = base.wait_for_serial("6.1.0-FipSOs", timeout=8.0, offset=pos_u)
        assert ok_u, f"Expected 6.1.0-FipSOs in uname -a:\n{base.read_serial()[pos_u:]}"
        print("[PASS] uname -a reports 6.1.0-FipSOs")

        # Wait briefly so prompt stabilizes before baseline frame snapshot
        ok_p0, _ = base.wait_for_serial("fipsos:/#", timeout=6.0, offset=pos_u)
        assert ok_p0, "Shell prompt missing after uname"
        used_base, free_base, c1_idle_base, hw0 = query_hwmon_frames(qmp, "baseline")
        print(f"[INFO] Baseline RAM: used={used_base} fr, free={free_base} fr, core1_idle={c1_idle_base}")
        assert "2 nucleos" in hw0 or "2 hilos logicos" in hw0, f"Unexpected SMP topology in hwmon:\n{hw0}"

        # Execute 4 child ELF programs back-to-back from shell and verify frame reclamation
        for idx in range(4):
            prog = "/bin/npu_infer.elf" if (idx % 2 == 0) else "/bin/c_std_app.elf"
            pos_c = len(base.read_serial())
            qmp.send_text(f"{prog}\n")
            ok_c, _ = base.wait_for_serial("sys_exit/exit_group(status=0)", timeout=10.0, offset=pos_c)
            assert ok_c, f"Child {prog} #{idx+1} did not exit cleanly:\n{base.read_serial()[pos_c:]}"
            ok_pr, _ = base.wait_for_serial("fipsos:/#", timeout=6.0, offset=pos_c)
            assert ok_pr, f"Shell did not return after {prog} #{idx+1}"

        used_post_sh, free_post_sh, c1_idle_post_sh, _ = query_hwmon_frames(qmp, "post_shell_execs")
        delta_sh = abs(free_post_sh - free_base)
        print(f"[PASS] 4x shell ELFs exited cleanly | free={free_post_sh} fr (delta={free_post_sh - free_base:+d} fr, c1_idle={c1_idle_post_sh})")
        assert delta_sh <= 8, f"Frame leak detected after 4x shell ELFs: delta={free_post_sh - free_base} fr"
        assert c1_idle_post_sh > c1_idle_base, f"AP Core 1 ticks stalled ({c1_idle_base} -> {c1_idle_post_sh})"

        # Verify repeated /dev/null opens/closes and real TCP socket open/close reclamation
        for idx in range(3):
            pos_s = len(base.read_serial())
            qmp.send_text("/bin/sandbox_probe\n")
            ok_s, _ = base.wait_for_serial("fipsos:/#", timeout=10.0, offset=pos_s)
            out_s = base.read_serial()[pos_s:]
            assert ok_s and "[probe] socket: ALLOWED" in out_s, f"sandbox_probe #{idx+1} failed:\n{out_s}"
        for idx in range(4):
            pos_n = len(base.read_serial())
            qmp.send_text("cat /dev/null\n")
            ok_n, _ = base.wait_for_serial("fipsos:/#", timeout=6.0, offset=pos_n)
            assert ok_n, f"cat /dev/null #{idx+1} failed"

        # Compile focused Ring 3 stress probe (16x socket+/dev/null close, RAM probe -EINVAL, fork+#PF, fork+SIGKILL)
        stress_cmds = [
            "echo '#include <stdio.h>' > /disk/v61_s.c",
            "echo '#include <fcntl.h>' >> /disk/v61_s.c",
            "echo '#include <unistd.h>' >> /disk/v61_s.c",
            "echo 'long sc3(long n,long a,long b,long c){long r;__asm__ __volatile__(\"syscall\":\"=a\"(r):\"a\"(n),\"D\"(a),\"S\"(b),\"d\"(c):\"rcx\",\"r11\",\"memory\");return r;}' >> /disk/v61_s.c",
            "echo 'int main(){int i;for(i=0;i<16;i++){int fd=open(\"/dev/null\",0);if(fd>=0)close(fd);long s=sc3(41,2,1,0);if(s>=0)close((int)s);}' >> /disk/v61_s.c",
            "echo 'int fb=open(\"/dev/fb0\",2);unsigned long rq[4]={99999L,0,0,0};' >> /disk/v61_s.c",
            "echo 'long inv=sc3(16,fb,0xC0105305L,(long)rq);if(fb>=0)close(fb);' >> /disk/v61_s.c",
            "echo 'if(inv+22){printf(\"BAD_INV=%ld\\n\",inv);return 1;}int p1=fork();if(p1==0){*(volatile int*)0=1;_exit(9);}' >> /disk/v61_s.c",
            "echo 'int st=0;sc3(61,p1,(long)&st,0);int p2=fork();if(p2==0){sc3(62,getpid(),9,0);_exit(9);}' >> /disk/v61_s.c",
            "echo 'sc3(61,p2,(long)&st,0);puts(\"V61_STRESS_OK\");return 0;}' >> /disk/v61_s.c",
            "/bin/tcc /disk/v61_s.c -o /disk/v61_s.elf",
            "echo 'long sc2(long n,long a,long b){long r;__asm__ __volatile__(\"syscall\":\"=a\"(r):\"a\"(n),\"D\"(a),\"S\"(b):\"rcx\",\"r11\",\"memory\");return r;}int main(){sc2(62,sc2(39,0,0),9);return 0;}' > /disk/v61_k.c",
            "/bin/tcc /disk/v61_k.c -o /disk/v61_k.elf",
            "echo 'int main(){*(volatile int*)0=42;return 0;}' > /disk/v61_f.c",
            "/bin/tcc /disk/v61_f.c -o /disk/v61_f.elf",
        ]
        for cmd in stress_cmds:
            pos_t = len(base.read_serial())
            send_shell_text(qmp, cmd + "\n")
            ok_t, _ = base.wait_for_serial("fipsos:/#", timeout=20.0, offset=pos_t)
            assert ok_t, f"TCC build command timed out: {cmd}\n{base.read_serial()[pos_t:]}"

        _, free_pre_stress, _, _ = query_hwmon_frames(qmp, "pre_stress")
        pos_r = len(base.read_serial())
        qmp.send_text("/disk/v61_s.elf\n")
        ok_r, _ = base.wait_for_serial("V61_STRESS_OK", timeout=12.0, offset=pos_r)
        assert ok_r, f"v61_s.elf did not print V61_STRESS_OK:\n{base.read_serial()[pos_r:]}"
        ok_rp, _ = base.wait_for_serial("fipsos:/#", timeout=8.0, offset=pos_r)
        assert ok_rp, "Shell missing after v61_s.elf"

        # Exercise exec'd child self-SIGKILL and exec'd child #PF recovery + exit reclamation
        for bin_path in ("/disk/v61_k.elf", "/disk/v61_f.elf"):
            pos_e = len(base.read_serial())
            qmp.send_text(f"{bin_path}\n")
            ok_e, _ = base.wait_for_serial("fipsos:/#", timeout=10.0, offset=pos_e)
            assert ok_e, f"Shell did not recover after {bin_path}:\n{base.read_serial()[pos_e:]}"

        _, free_post_stress, _, _ = query_hwmon_frames(qmp, "post_stress")
        delta_stress = abs(free_post_stress - free_pre_stress)
        print(f"[PASS] Socket/devnull/EINVAL/#PF/SIGKILL stress passed | free={free_post_stress} fr (delta={free_post_stress - free_pre_stress:+d} fr)")
        assert delta_stress <= 8, f"Frame leak detected after socket/devnull/fault/SIGKILL stress: delta={free_post_stress - free_pre_stress} fr"
        free_post_sh = free_post_stress

        # Launch GUI v6.1
        time.sleep(0.2)
        qmp.send_text("gui\n")
        ok_gui, _ = base.wait_for_serial("[desktop_ui.elf] FipSOs v6.1", timeout=15.0)
        assert ok_gui, f"GUI v6.1 did not launch:\n{base.read_serial()[-800:]}"
        time.sleep(0.9)

        shot_clean = qmp.screendump("shot_v61_clean_desktop")
        ppm_clean = os.path.join(REPO_DIR, "build", "shot_v61_clean_desktop.ppm")
        ppm_files.append(ppm_clean)
        w, h, clean_rgb = load_ppm_rgb(ppm_clean)
        qmp.mouse_x = w // 2
        qmp.mouse_y = h // 2
        print(f"[PASS] Clean desktop rendered at {w}x{h} -> {shot_clean}")

        # Open Live Kernel Monitor (wins[0]) via Dock Icon #1
        taskbar_y = h - 48
        dock_x0 = (w - 384) // 2
        qmp.click_at(dock_x0 + 48 + 20, taskbar_y + 24)
        time.sleep(0.7)

        shot_open = qmp.screendump("shot_v61_live_monitor_open")
        ppm_open = os.path.join(REPO_DIR, "build", "shot_v61_live_monitor_open.ppm")
        ppm_files.append(ppm_open)
        _, _, open_rgb = load_ppm_rgb(ppm_open)
        assert region_signature(clean_rgb, w, 40, 60, 560, 320) != region_signature(
            open_rgb, w, 40, 60, 560, 320
        ), "Live Kernel Monitor window (wins[0]) did not paint on Dock click!"
        print(f"[PASS] Live Kernel Monitor (wins[0]) opened -> {shot_open}")

        # Verify live tick progression inside SMP/IRQ0 box without user clicks (~1.1s elapsed)
        time.sleep(1.1)
        shot_tick = qmp.screendump("shot_v61_live_tick_progress")
        ppm_tick = os.path.join(REPO_DIR, "build", "shot_v61_live_tick_progress.ppm")
        ppm_files.append(ppm_tick)
        _, _, tick_rgb = load_ppm_rgb(ppm_tick)
        assert region_signature(open_rgb, w, 38, 66, 610, 94) != region_signature(
            tick_rgb, w, 38, 66, 610, 94
        ), "Live Kernel Monitor SMP/IRQ0 box did not update automatically!"
        print(f"[PASS] Live SMP/IRQ0 counters updated autonomously -> {shot_tick}")

        # Click "Probar RAM R/W" (wins[0] = (24, 24, 656, 420), button 0 at (38, 408, 154, 26))
        qmp.click_at(115, 421)
        time.sleep(0.55)
        shot_ram = qmp.screendump("shot_v61_ram_rw_probe")
        ppm_ram = os.path.join(REPO_DIR, "build", "shot_v61_ram_rw_probe.ppm")
        ppm_files.append(ppm_ram)
        _, _, ram_rgb = load_ppm_rgb(ppm_ram)
        # RAM R/W status line sits at y = 24 + 42 + 98 + 74 = 238
        assert region_signature(tick_rgb, w, 40, 232, 560, 18) != region_signature(
            ram_rgb, w, 40, 232, 560, 18
        ), "Clicking 'Probar RAM R/W' did not update HHDM probe result line!"
        print(f"[PASS] HHDM RAM R/W probe button executed & painted MiB/s telemetry -> {shot_ram}")

        # Click "Lanzar Proceso" (button 1 at (196, 408, 154, 26)) 4 times back-to-back
        for seq in range(4):
            pos_l = len(base.read_serial())
            qmp.click_at(273, 421)
            ok_exit, _ = base.wait_for_serial("sys_exit/exit_group(status=0)", timeout=12.0, offset=pos_l)
            assert ok_exit, f"GUI 'Lanzar Proceso' #{seq+1} failed/hung:\n{base.read_serial()[pos_l:]}"
            ok_fb, _ = base.wait_for_serial("FB_IOCTL_SET_DESKTOP_MODE(enabled=true)", timeout=8.0, offset=pos_l)
            assert ok_fb, f"Desktop mode not restored after GUI child #{seq+1}:\n{base.read_serial()[pos_l:]}"
            time.sleep(0.25)

        shot_launches = qmp.screendump("shot_v61_after_launches")
        ppm_launches = os.path.join(REPO_DIR, "build", "shot_v61_after_launches.ppm")
        ppm_files.append(ppm_launches)
        _, _, launches_rgb = load_ppm_rgb(ppm_launches)
        # Process box header shows "Lanzamientos=4" at y = 24 + 42 + 194 + 7 = 267
        assert region_signature(ram_rgb, w, 40, 260, 560, 22) != region_signature(
            launches_rgb, w, 40, 260, 560, 22
        ), "Process box header/rows did not reflect 4 child launches!"
        print(f"[PASS] 4x GUI 'Lanzar Proceso' completed with zero hang/exhaustion -> {shot_launches}")

        # Click "Ciclar Perfil" (431, 421) and "Refrescar Vivo" (589, 421)
        qmp.click_at(431, 421)
        time.sleep(0.35)
        qmp.click_at(589, 421)
        time.sleep(0.45)
        shot_final = qmp.screendump("shot_v61_final_panel")
        ppm_final = os.path.join(REPO_DIR, "build", "shot_v61_final_panel.ppm")
        ppm_files.append(ppm_final)
        _, _, final_rgb = load_ppm_rgb(ppm_final)
        assert region_signature(launches_rgb, w, 354, 408, 154, 26) != region_signature(
            final_rgb, w, 354, 408, 154, 26
        ), "'Ciclar Perfil' button did not update profile label!"
        print(f"[PASS] 'Ciclar Perfil' + 'Refrescar Vivo' updated live panel -> {shot_final}")

        # Exit Desktop cleanly back to TTY via 'q'
        pos_q = len(base.read_serial())
        qmp.send_text("q")
        ok_q, _ = base.wait_for_serial("fipsos:/#", timeout=12.0, offset=pos_q)
        tail_q = base.read_serial()[pos_q:]
        assert ok_q and "Sesion grafica finalizada limpiamente" in tail_q, f"Clean GUI exit failed:\n{tail_q}"
        print("[PASS] Exited GUI cleanly back to fipsos:/#")

        used_end, free_end, c1_idle_end, _ = query_hwmon_frames(qmp, "post_gui")
        delta_end = abs(free_end - free_post_sh)
        print(f"[PASS] Post-GUI RAM: used={used_end} fr, free={free_end} fr (delta_vs_pre_gui={free_end - free_post_sh:+d} fr, delta_vs_base={free_end - free_base:+d} fr, c1_idle={c1_idle_end})")
        assert delta_end <= 8, f"Frame leak after GUI + RAM R/W probe + 4 child launches: delta={free_end - free_post_sh} fr"

        full_log = base.strip_ansi(base.read_serial())
        assert "PANIC" not in full_log and "Triple" not in full_log, "Kernel panic/fault detected in serial log!"
        print("=== ALL FipSOs v6.1 LIVE TELEMETRY & RECLAMATION TESTS PASSED ===")
        return 0
    finally:
        if qmp is not None:
            try:
                qmp.close()
            except Exception:
                pass
        proc.terminate()
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()
        for temp in [DISK_PATH, SERIAL_LOG] + ppm_files:
            if os.path.exists(temp):
                try:
                    os.remove(temp)
                except OSError:
                    pass


if __name__ == "__main__":
    sys.exit(main())
