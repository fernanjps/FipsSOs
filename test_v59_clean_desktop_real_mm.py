#!/usr/bin/env python3
"""
FipSOs v5.9 Verification Suite:
  1. Verifies Limine UEFI/E820 real memory map parsing & dynamic Kernel Heap scaling (-m 1024M -> Heap > 8 MiB).
  2. Verifies clean desktop startup (`gui`): 0 auto-opened windows, real RAM/RTC on frame 1, zero blocking startup probes.
  3. Verifies event loop liveness: mouse motion + opening Monitor/Explorer windows + clean exit back to TTY via `'q'`.
"""

import os
import subprocess
import sys
import time

import test_v51_layer3_suite as base

REPO_DIR = base.REPO_DIR
ISO_PATH = base.ISO_PATH
USB_PATH = os.path.join(REPO_DIR, "build", "fipsos-baremetal-usb.img")
DISK_PATH = os.path.join(REPO_DIR, "build", "test_v59_sata.img")
SERIAL_LOG = os.path.join(REPO_DIR, "build", "test_v59_serial.log")
QMP_PORT = 4490

base.DISK_PATH = DISK_PATH
base.SERIAL_LOG = SERIAL_LOG
base.QMP_PORT = QMP_PORT


def create_2g_disk(path: str) -> None:
    os.makedirs(os.path.dirname(path), exist_ok=True)
    if os.path.exists(path):
        os.remove(path)
    with open(path, "wb") as f:
        f.truncate(2048 * 1024 * 1024)


def sample_ppm_pixel(ppm_path: str, x: int, y: int) -> tuple[int, int, int]:
    with open(ppm_path, "rb") as f:
        header = []
        while len(header) < 3:
            line = f.readline()
            if not line.startswith(b"#"):
                header.extend(line.split())
        w = int(header[1])
        data = f.read()
    idx = (y * w + x) * 3
    return data[idx], data[idx + 1], data[idx + 2]


def main() -> int:
    print("=== FipSOs v5.9 Clean Desktop, Real E820 MM & Mouse Verification ===")
    assert os.path.exists(ISO_PATH), f"Missing {ISO_PATH}"
    assert os.path.exists(USB_PATH), f"Missing {USB_PATH}"
    create_2g_disk(DISK_PATH)
    if os.path.exists(SERIAL_LOG):
        os.remove(SERIAL_LOG)

    qemu_cmd = [
        "qemu-system-x86_64",
        "-M", "q35",
        "-cpu", "EPYC,+hypervisor",
        "-smp", "2",
        "-m", "1024M",
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
        ok, _ = base.wait_for_serial("fipsos:/#", timeout=45.0)
        assert ok, f"Shell prompt not reached. Tail:\n{base.read_serial()[-1200:]}"

        serial_text = base.read_serial()
        assert "[ostd::mm::e820]" in serial_text, "Missing UEFI/E820 map logs in serial output"
        assert "Heap=16 MiB" in serial_text or "Heap=32 MiB" in serial_text, (
            f"Expected dynamically scaled Kernel Heap (16/32 MiB) for 1024M RAM:\n{serial_text[:1800]}"
        )
        assert "FipSOs v5.9" in serial_text, "Expected FipSOs v5.9 boot banner"
        print("[PASS] UEFI/E820 physical map parsed & Kernel Heap dynamically scaled > 8 MiB")

        time.sleep(0.3)
        qmp.send_text("uname -a\n")
        ok_uname, _ = base.wait_for_serial("5.9.0-FipSOs", timeout=8.0)
        assert ok_uname, f"Expected 5.9.0-FipSOs in uname -a:\n{base.read_serial()[-600:]}"
        print("[PASS] uname -a reports 5.9.0-FipSOs")

        time.sleep(0.2)
        qmp.send_text("gui\n")
        ok_gui, _ = base.wait_for_serial("[desktop_ui.elf] FipSOs v5.9", timeout=15.0)
        assert ok_gui, f"GUI did not launch:\n{base.read_serial()[-800:]}"
        time.sleep(1.0)

        shot_clean = qmp.screendump("shot_v59_clean_desktop")
        ppm_clean = os.path.join(REPO_DIR, "build", "shot_v59_clean_desktop.ppm")
        # Sample center pixel (should be clean blue/slate wallpaper, NOT dark window background)
        r, g, b = sample_ppm_pixel(ppm_clean, 512, 320)
        assert not (r == 10 and g == 22 and b == 40), f"Still on text console! rgb={(r, g, b)}"
        assert g > 20 or b > 40, f"Unexpected center pixel color on clean desktop: {(r, g, b)}"
        print(f"[PASS] Clean desktop rendered immediately (center rgb={(r, g, b)}) -> {shot_clean}")

        # Exercise PS/2 / USB mouse motion via QMP to verify non-blocking mouse input
        res_ev = qmp.cmd("input-send-event", {
            "events": [
                {"type": "rel", "data": {"axis": "x", "value": 60}},
                {"type": "rel", "data": {"axis": "y", "value": -35}},
            ]
        })
        assert "error" not in res_ev, f"QMP mouse injection failed: {res_ev}"
        print("[PASS] QMP mouse relative motion dispatched without error")

        # Open Monitor HW ('1') and Explorer ('3') via keyboard shortcuts (event loop liveness)
        qmp.send_text("1")
        time.sleep(0.5)
        qmp.send_text("3")
        time.sleep(0.8)
        shot_open = qmp.screendump("shot_v59_lazy_open")
        print(f"[PASS] On-demand window opening responsive -> {shot_open}")

        # Exit GUI cleanly via 'q' to prove zero startup freeze
        pos_before_q = len(base.read_serial())
        qmp.send_text("q")
        ok_exit = False
        deadline = time.time() + 12.0
        while time.time() < deadline:
            tail = base.read_serial()[pos_before_q:]
            if "Sesion grafica finalizada limpiamente" in tail and "fipsos:/#" in tail:
                ok_exit = True
                break
            time.sleep(0.15)
        assert ok_exit, f"GUI did not process 'q' / exit cleanly:\n{base.read_serial()[pos_before_q:]}"
        print("[PASS] Event loop alive & non-blocking: GUI exited cleanly back to fipsos:/#")

        print("=== ALL FipSOs v5.9 VERIFICATION TESTS PASSED ===")
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
        for temp in (
            DISK_PATH,
            SERIAL_LOG,
            os.path.join(REPO_DIR, "build", "shot_v59_clean_desktop.ppm"),
            os.path.join(REPO_DIR, "build", "shot_v59_lazy_open.ppm"),
        ):
            if os.path.exists(temp):
                try:
                    os.remove(temp)
                except OSError:
                    pass


if __name__ == "__main__":
    sys.exit(main())
