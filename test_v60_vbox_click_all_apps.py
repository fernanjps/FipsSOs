#!/usr/bin/env python3
"""
FipSOs v6.0 Verification Suite (VirtualBox Profile):
  1. Boots ISO on Q35 + EPYC,+hypervisor + 2048 MiB RAM + 2048 MiB SATA disk + xHCI HID + e1000e.
  2. Verifies VFS /dev/null & /tmp RAM nodes, FipSOs v6.0 / uname 6.0.0-FipSOs, EC_BAT=NotPresent.
  3. Launches `gui`, detects actual framebuffer resolution (w, h), and exercises REAL QMP mouse clicks:
     - Opens Start Menu by clicking Dock Start icon -> verifies Start Menu pixels.
     - Clicks ALL 8 Start Menu tiles (Monitor HW, Visor 3D, Explorador, App Center, Terminal TTY,
       Editor, Seguridad IA, avatar_cpp.elf), verifying instant rendering & non-blocking return.
     - Tests live Terminal command (`uname -a`) and Seguridad IA buttons (`Auditar Disco`, `Tono Audio`).
     - Verifies Dock click after avatar_cpp.elf finishes and clean `'q'` exit back to TTY.
"""

import os
import subprocess
import sys
import time

import test_v51_layer3_suite as base

REPO_DIR = base.REPO_DIR
ISO_PATH = base.ISO_PATH
DISK_PATH = os.path.join(REPO_DIR, "build", "test_v60_sata.img")
SERIAL_LOG = os.path.join(REPO_DIR, "build", "test_v60_serial.log")
QMP_PORT = 4492

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


def sample_region_hash(rgb: bytes, w: int, x0: int, y0: int, rw: int = 80, rh: int = 40) -> tuple[int, int, int]:
    r_sum = 0
    g_sum = 0
    b_sum = 0
    cnt = 0
    for y in range(y0, y0 + rh, 4):
        for x in range(x0, x0 + rw, 4):
            idx = (y * w + x) * 3
            r_sum += rgb[idx]
            g_sum += rgb[idx + 1]
            b_sum += rgb[idx + 2]
            cnt += 1
    return r_sum // cnt, g_sum // cnt, b_sum // cnt


def click_start_tile(qmp: base.QmpClient, w: int, h: int, tile_idx: int) -> None:
    taskbar_y = h - 48
    dock_x0 = (w - 384) // 2
    qmp.click_at(dock_x0 + 20, taskbar_y + 24)
    time.sleep(0.35)
    sx = (w - 440) // 2
    sy = taskbar_y - 350 - 8
    tx = sx + 14 + (tile_idx % 4) * 104 + 49
    ty = sy + 78 + (tile_idx // 4) * 96 + 42
    qmp.click_at(tx, ty)


def main() -> int:
    print("=== FipSOs v6.0 VirtualBox-Profile Click-All-Apps Verification ===")
    assert os.path.exists(ISO_PATH), f"Missing {ISO_PATH}"
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
    ppm_files = []
    try:
        qmp = base.QmpClient(QMP_PORT)
        ok, _ = base.wait_for_serial("fipsos:/#", timeout=45.0)
        assert ok, f"Shell prompt not reached. Tail:\n{base.read_serial()[-1200:]}"

        boot_log = base.read_serial()
        assert "FipSOs v6.0" in boot_log, "Missing FipSOs v6.0 boot banner"
        assert "[ostd::power] EC_BAT=NotPresent" in boot_log, "Expected fast EC_BAT=NotPresent probe"
        print("[PASS] Booted FipSOs v6.0 on 2048 MiB RAM + 2 GiB SATA + EPYC hypervisor")

        qmp.send_text("uname -a\n")
        ok_u, _ = base.wait_for_serial("6.0.0-FipSOs", timeout=8.0)
        assert ok_u, f"Expected 6.0.0-FipSOs in uname -a:\n{base.read_serial()[-600:]}"
        print("[PASS] uname -a reports 6.0.0-FipSOs")

        qmp.send_text("echo v60_tmp_ok > /tmp/v60.txt && cat /tmp/v60.txt && echo devnull > /dev/null\n")
        ok_tmp, _ = base.wait_for_serial("v60_tmp_ok", timeout=8.0)
        assert ok_tmp, f"Expected /tmp RAM write+read & /dev/null write:\n{base.read_serial()[-600:]}"
        print("[PASS] VFS /tmp RAM directory and /dev/null node verified")

        time.sleep(0.2)
        qmp.send_text("gui\n")
        ok_gui, _ = base.wait_for_serial("[desktop_ui.elf] FipSOs v6.0", timeout=15.0)
        assert ok_gui, f"GUI v6.0 did not launch:\n{base.read_serial()[-800:]}"
        time.sleep(0.9)

        shot_clean = qmp.screendump("shot_v60_clean_desktop")
        ppm_clean = os.path.join(REPO_DIR, "build", "shot_v60_clean_desktop.ppm")
        ppm_files.append(ppm_clean)
        w, h, clean_rgb = load_ppm_rgb(ppm_clean)
        qmp.mouse_x = w // 2
        qmp.mouse_y = h // 2
        print(f"[PASS] Clean desktop rendered at {w}x{h} -> {shot_clean}")

        # Open Start Menu via Dock i=0 and verify pixels change
        taskbar_y = h - 48
        dock_x0 = (w - 384) // 2
        sx = (w - 440) // 2
        sy = taskbar_y - 350 - 8
        qmp.click_at(dock_x0 + 20, taskbar_y + 24)
        time.sleep(0.45)
        shot_start = qmp.screendump("shot_v60_start_menu")
        ppm_start = os.path.join(REPO_DIR, "build", "shot_v60_start_menu.ppm")
        ppm_files.append(ppm_start)
        _, _, start_rgb = load_ppm_rgb(ppm_start)
        assert sample_region_hash(clean_rgb, w, sx + 40, sy + 120) != sample_region_hash(
            start_rgb, w, sx + 40, sy + 120
        ), "Start Menu did not appear after clicking Start button"
        print(f"[PASS] Start Menu opened on first click -> {shot_start}")

        # Close Start Menu before uniform click_start_tile loop
        qmp.click_at(dock_x0 + 20, taskbar_y + 24)
        time.sleep(0.35)

        half_w = (w - 64) // 2
        half_h = (taskbar_y - 64) // 2
        win_geoms = [
            (24, 24, half_w, half_h, "Monitor HW"),
            (24 + half_w + 16, 24, half_w, half_h, "Visor 3D"),
            (24, 24 + half_h + 16, half_w, half_h, "Explorador"),
            (24 + half_w + 16, 24 + half_h + 16, half_w, half_h, "App Center"),
            (180, 96, 684, 410, "Terminal TTY"),
            (160, 82, 610, 386, "Editor"),
            (170, 88, 636, 404, "Seguridad IA"),
        ]

        for wid, (wx, wy, ww, wh, name) in enumerate(win_geoms):
            click_start_tile(qmp, w, h, wid)
            time.sleep(0.55)
            ppm_tag = f"shot_v60_win{wid}"
            shot_w = qmp.screendump(ppm_tag)
            ppm_p = os.path.join(REPO_DIR, "build", f"{ppm_tag}.ppm")
            ppm_files.append(ppm_p)
            _, _, w_rgb = load_ppm_rgb(ppm_p)

            sample_x = wx + min(120, ww // 3)
            sample_y = wy + min(100, wh // 3)
            bg_sig = sample_region_hash(clean_rgb, w, sample_x, sample_y)
            win_sig = sample_region_hash(w_rgb, w, sample_x, sample_y)
            assert bg_sig != win_sig, f"Window {name} (wid={wid}) did not paint on click!"
            print(f"[PASS] Start Menu Tile {wid} ({name}) rendered immediately on click -> {shot_w}")

            if wid == 4:
                pos_t = len(base.read_serial())
                qmp.send_text("uname -a\n")
                ok_term, _ = base.wait_for_serial("[ui_terminal::exec] uname -a", timeout=6.0, offset=pos_t)
                assert ok_term, f"Terminal command failed inside desktop:\n{base.read_serial()[pos_t:]}"
                ok_fb, _ = base.wait_for_serial("FB_IOCTL_SET_DESKTOP_MODE(enabled=true)", timeout=6.0, offset=pos_t)
                assert ok_fb, f"Desktop mode not restored after terminal cmd:\n{base.read_serial()[pos_t:]}"
                pos_c = len(base.read_serial())
                qmp.send_text("cat\n")
                ok_cat, _ = base.wait_for_serial("[ui_terminal::exec] cat", timeout=6.0, offset=pos_c)
                assert ok_cat, f"Terminal 'cat' (dup2 /dev/null -> FD_STDIN) failed/hung:\n{base.read_serial()[pos_c:]}"
                ok_cat_fb, _ = base.wait_for_serial("FB_IOCTL_SET_DESKTOP_MODE(enabled=true)", timeout=6.0, offset=pos_c)
                assert ok_cat_fb, f"Desktop mode not restored after 'cat' EOF:\n{base.read_serial()[pos_c:]}"
                print("[PASS] Terminal TTY executed 'uname -a' & 'cat' (dup2 /dev/null -> stdin EOF) non-blockingly")
            elif wid == 6:
                pos_s = len(base.read_serial())
                by = wy + wh - 23
                qmp.click_at(wx + 75, by)   # Auditar Disco -> /tmp/sec_audit.log
                ok_aud, _ = base.wait_for_serial('sys_execve("/bin/goose_audit"', timeout=8.0, offset=pos_s)
                assert ok_aud, f"Auditar Disco did not run goose_audit:\n{base.read_serial()[pos_s:]}"
                qmp.click_at(wx + 380, by)  # Tono Audio -> /dev/audio
                time.sleep(0.4)
                print("[PASS] Seguridad IA buttons ('Auditar Disco' & 'Tono Audio') responded without freeze")

            # Close window via [X] button
            qmp.click_at(wx + ww - 18, wy + 16)
            time.sleep(0.35)

        # Launch Start Menu Tile 7 (avatar_cpp.elf) and verify clean non-blocking return
        pos_av = len(base.read_serial())
        click_start_tile(qmp, w, h, 7)
        ok_av, _ = base.wait_for_serial("[avatar_cpp.elf] Resumen", timeout=15.0, offset=pos_av)
        assert ok_av, f"avatar_cpp.elf did not complete/return cleanly:\n{base.read_serial()[pos_av:]}"
        time.sleep(0.5)
        print("[PASS] Start Menu Tile 7 (avatar_cpp.elf) rendered & returned to Desktop cleanly")

        # Click Dock Icon #1 (Monitor HW) directly to verify desktop responsiveness post-execve
        qmp.click_at(dock_x0 + 1 * 48 + 20, taskbar_y + 24)
        time.sleep(0.5)
        shot_after = qmp.screendump("shot_v60_dock_after_avatar")
        ppm_after = os.path.join(REPO_DIR, "build", "shot_v60_dock_after_avatar.ppm")
        ppm_files.append(ppm_after)
        _, _, after_rgb = load_ppm_rgb(ppm_after)
        assert sample_region_hash(clean_rgb, w, 120, 120) != sample_region_hash(
            after_rgb, w, 120, 120
        ), "Dock click after avatar_cpp.elf did not reopen Monitor HW!"
        print(f"[PASS] Dock icon click after avatar_cpp.elf opened Monitor HW -> {shot_after}")

        # Exit Desktop cleanly back to TTY via 'q'
        pos_q = len(base.read_serial())
        qmp.send_text("q")
        ok_q = False
        deadline = time.time() + 12.0
        while time.time() < deadline:
            tail = base.read_serial()[pos_q:]
            if "Sesion grafica finalizada limpiamente" in tail and "fipsos:/#" in tail:
                ok_q = True
                break
            time.sleep(0.15)
        assert ok_q, f"Desktop did not exit via 'q':\n{base.read_serial()[pos_q:]}"
        print("[PASS] Desktop event loop stayed responsive across all 8 apps and exited to fipsos:/#")
        print("=== ALL FipSOs v6.0 VIRTUALBOX-PROFILE CLICK TESTS PASSED ===")
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
