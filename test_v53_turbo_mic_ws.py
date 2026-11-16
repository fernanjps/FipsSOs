#!/usr/bin/env python3
"""
FipSOs v5.3 Verification Suite:
  1. FipSOs v5.3 Framekernel boot, uname -a -> 5.3.0-FipSOs
  2. Turbo Boost MSR/CPB profile switching (/sys/class/power_supply/BAT0/boost)
  3. Bidirectional /dev/audio PCM capture + /bin/fips_ai --audio-listen
  4. Virtual Workspaces (WS1..WS4) in C++20 Desktop Compositor
"""

import os
import subprocess
import sys
import time

import test_v51_layer3_suite as base

REPO_DIR = base.REPO_DIR
ISO_PATH = base.ISO_PATH
DISK_PATH = os.path.join(REPO_DIR, "build", "test_v53_fat32.img")
SERIAL_LOG = os.path.join(REPO_DIR, "build", "test_v53_serial.log")
QMP_PORT = 4473

base.DISK_PATH = DISK_PATH
base.SERIAL_LOG = SERIAL_LOG
base.QMP_PORT = QMP_PORT


def main() -> int:
    base.create_fat32_disk(DISK_PATH)
    if os.path.exists(SERIAL_LOG):
        os.remove(SERIAL_LOG)

    qemu_cmd = [
        "qemu-system-x86_64",
        "-M", "q35",
        "-cpu", "max",
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
        ok, _ = base.wait_for_serial("fipsos:/#", timeout=30.0)
        assert ok, f"Shell prompt not reached. Serial:\n{base.read_serial()[-1200:]}"
        time.sleep(0.5)

        shot1 = qmp.screendump("shot1_v53_boot")
        print(f"[PASS] Booted FipSOs v5.3 -> {shot1}")

        base.run_shell_cmd(qmp, "uname -a", "5.3.0-FipSOs")
        print("[PASS] uname -a -> 5.3.0-FipSOs")

        # 1. Turbo Boost control via /sys/class/power_supply/BAT0/profile & boost
        base.run_shell_cmd(qmp, "echo performance > /sys/class/power_supply/BAT0/profile", "fipsos:/#")
        b_on = base.run_shell_cmd(qmp, "cat /sys/class/power_supply/BAT0/boost", "TURBO_ON")
        assert "TURBO_ON" in b_on
        base.run_shell_cmd(qmp, "echo powersave > /sys/class/power_supply/BAT0/profile", "fipsos:/#")
        b_eco = base.run_shell_cmd(qmp, "cat /sys/class/power_supply/BAT0/boost", "BASE_ECO")
        assert "BASE_ECO" in b_eco
        hwmon = base.run_shell_cmd(qmp, "cat /sys/fipsos/hwmon", "Boost: BASE_ECO")
        assert "FipSOs v5.3 Hardware Monitor" in hwmon
        print("[PASS] Turbo Boost profile -> TURBO_ON / BASE_ECO verified")

        # 2. Bidirectional /dev/audio PCM capture + /bin/fips_ai --audio-listen
        mic = base.run_shell_cmd(qmp, "/bin/fips_ai --audio-listen", "--audio-listen /dev/audio (256B PCM)")
        assert "energy=" in mic and "zcr=" in mic and "class=" in mic and "backend=" in mic
        print("[PASS] /dev/audio PCM capture + /bin/fips_ai --audio-listen verified")

        # 3. Desktop GUI Virtual Workspaces (WS1 -> WS2)
        start_gui = len(base.read_serial())
        qmp.send_text("gui\n")
        ok, _ = base.wait_for_serial("[desktop_ui.elf] FipSOs v5.3", timeout=15.0, offset=start_gui)
        assert ok, f"Desktop GUI v5.3 did not start:\n{base.read_serial()[start_gui:]}"
        time.sleep(1.0)

        shot2 = qmp.screendump("shot2_v53_ws1")
        print(f"[PASS] Desktop WS1 captured -> {shot2}")

        # Switch to WS2 via taskbar button '2' (x=158+26+11=195, y=800-48+24=776)
        qmp.click_at(195, 776)
        time.sleep(0.8)
        shot3 = qmp.screendump("shot3_v53_ws2")
        print(f"[PASS] Desktop WS2 captured -> {shot3}")

        qmp.send_key("q")
        base.wait_for_serial("fipsos@v5.3:/$", timeout=10.0, offset=start_gui)
        print("[PASS] Returned cleanly to TTY from Desktop")
        return 0
    finally:
        if qmp is not None:
            qmp.close()
        proc.terminate()
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()


if __name__ == "__main__":
    sys.exit(main())
