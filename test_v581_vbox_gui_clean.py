#!/usr/bin/env python3
"""
FipSOs v5.8.1 Verification Suite:
  1. Verifies build/ is clean of obsolete images and has up-to-date ISO + USB image + docs/.
  2. Boots FipSOs in VirtualBox-profile QEMU (EPYC,+hypervisor, 2G SATA, xHCI, e1000e).
  3. Verifies Lua REPL exits cleanly on bare `exit`.
  4. Verifies `gui` immediately presents the first desktop frame to physical hardware VRAM
     (checking actual RGB pixel diversity in QMP screendump, not just VMO RAM).
"""

import glob
import os
import subprocess
import sys
import time

import test_v51_layer3_suite as base

REPO_DIR = base.REPO_DIR
ISO_PATH = base.ISO_PATH
USB_PATH = os.path.join(REPO_DIR, "build", "fipsos-baremetal-usb.img")
DISK_PATH = os.path.join(REPO_DIR, "build", "test_v581_sata.img")
SERIAL_LOG = os.path.join(REPO_DIR, "build", "test_v581_serial.log")
QMP_PORT = 4489

base.DISK_PATH = DISK_PATH
base.SERIAL_LOG = SERIAL_LOG
base.QMP_PORT = QMP_PORT


def create_2g_disk(path: str) -> None:
    os.makedirs(os.path.dirname(path), exist_ok=True)
    if os.path.exists(path):
        os.remove(path)
    with open(path, "wb") as f:
        f.truncate(2048 * 1024 * 1024)


def verify_repo_hygiene() -> None:
    build_dir = os.path.join(REPO_DIR, "build")
    obsolete = []
    for pat in ("*v36*.img", "*v37*.img", "*v40*.img", "*v41*.img", "*v42*.img", "*v43*.img", "*vbox*.img"):
        obsolete.extend(glob.glob(os.path.join(build_dir, pat)))
    assert not obsolete, f"Obsolete images found in build/: {obsolete}"
    assert os.path.exists(ISO_PATH), f"Missing {ISO_PATH}"
    assert os.path.exists(USB_PATH), f"Missing {USB_PATH}"
    assert os.path.getsize(USB_PATH) == os.path.getsize(ISO_PATH), "USB image out of sync with ISO"
    doc_path = os.path.join(REPO_DIR, "docs", "ARCHITECTURE_HISTORY.md")
    assert os.path.exists(doc_path), f"Missing {doc_path}"
    print("[PASS] Repository hygiene verified (build/ clean, ISO == USB image, docs/ARCHITECTURE_HISTORY.md present)")


def count_non_console_pixels(ppm_path: str) -> int:
    with open(ppm_path, "rb") as f:
        header = []
        while len(header) < 3:
            line = f.readline()
            if not line.startswith(b"#"):
                header.extend(line.split())
        data = f.read()
    # Sample top-left / desktop area pixels that are not console background (#0A1628) or black (#000000)
    non_bg = 0
    for i in range(0, min(len(data), 1024 * 600 * 3), 3 * 17):
        r, g, b = data[i], data[i + 1], data[i + 2]
        if (r, g, b) not in ((0, 0, 0), (10, 22, 40)):
            non_bg += 1
    return non_bg


def main() -> int:
    print("=== FipSOs v5.8.1 GUI Present, Lua Exit & Hygiene Verification ===")
    verify_repo_hygiene()
    create_2g_disk(DISK_PATH)
    if os.path.exists(SERIAL_LOG):
        os.remove(SERIAL_LOG)

    qemu_cmd = [
        "qemu-system-x86_64",
        "-M", "q35",
        "-cpu", "EPYC,+hypervisor",
        "-smp", "2",
        "-m", "512M",
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
    try:
        qmp = base.QmpClient(QMP_PORT)
        ok, _ = base.wait_for_serial("fipsos:/#", timeout=40.0)
        assert ok, f"Shell prompt not reached. Tail:\n{base.read_serial()[-1200:]}"
        time.sleep(0.4)

        # 1. Verify Lua REPL exits cleanly with bare `exit`
        qmp.send_text("/bin/lua\n")
        ok_lua, _ = base.wait_for_serial("Lua 5.4.7", timeout=10.0)
        assert ok_lua, f"Lua REPL did not start:\n{base.read_serial()[-800:]}"
        time.sleep(0.2)
        qmp.send_text("print(21+21)\n")
        ok_42, _ = base.wait_for_serial("42", timeout=8.0)
        assert ok_42, "Lua did not evaluate print(21+21)"
        pos_before_exit = len(base.read_serial())
        qmp.send_text("exit\n")
        ok_ret = False
        deadline = time.time() + 10.0
        while time.time() < deadline:
            if "fipsos:/#" in base.read_serial()[pos_before_exit:]:
                ok_ret = True
                break
            time.sleep(0.1)
        assert ok_ret, f"Lua did not exit on `exit`:\n{base.read_serial()[pos_before_exit:]}"
        print("[PASS] Lua REPL evaluated 21+21=42 and exited cleanly on `exit`")

        # 2. Launch `gui` and verify immediate first-frame hardware VRAM flush
        qmp.send_text("gui\n")
        ok_gui, _ = base.wait_for_serial("[desktop_ui.elf] FipSOs v5.8", timeout=15.0)
        assert ok_gui, f"GUI did not launch:\n{base.read_serial()[-800:]}"
        time.sleep(0.8)
        shot_gui = qmp.screendump("shot_v581_gui_first_frame")
        ppm_path = os.path.join(REPO_DIR, "build", "shot_v581_gui_first_frame.ppm")
        non_bg = count_non_console_pixels(ppm_path)
        assert non_bg > 500, f"Framebuffer still looks like blank/text console! non_bg={non_bg}"
        print(f"[PASS] GUI first frame flushed to physical VRAM (non_bg={non_bg}) -> {shot_gui}")

        print("=== ALL FipSOs v5.8.1 TESTS PASSED ===")
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
        for temp in (DISK_PATH, SERIAL_LOG, os.path.join(REPO_DIR, "build", "shot_v581_gui_first_frame.ppm")):
            if os.path.exists(temp):
                try:
                    os.remove(temp)
                except OSError:
                    pass


if __name__ == "__main__":
    sys.exit(main())
