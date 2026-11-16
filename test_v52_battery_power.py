#!/usr/bin/env python3
"""
FipSOs v5.2 Battery & Power Management Verification Suite:
  1. FipSOs v5.2 Framekernel boot, uname -a -> 5.2.0-FipSOs
  2. /sys/class/power_supply/BAT0/{capacity,status,profile} exposure
  3. Power profile switching via /sys/class/power_supply/BAT0/profile
  4. /sys/fipsos/hwmon [PWR] telemetry line
  5. C++20 Desktop taskbar BAT indicator & Monitor del Sistema power profile button
"""

import os
import subprocess
import sys
import time

import test_v51_layer3_suite as base

REPO_DIR = base.REPO_DIR
ISO_PATH = base.ISO_PATH
DISK_PATH = os.path.join(REPO_DIR, "build", "test_v52_fat32.img")
SERIAL_LOG = os.path.join(REPO_DIR, "build", "test_v52_serial.log")
QMP_PORT = 4472

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

        shot1 = qmp.screendump("shot1_v52_boot")
        print(f"[PASS] Booted FipSOs v5.2 -> {shot1}")

        base.run_shell_cmd(qmp, "uname -a", "5.2.0-FipSOs")
        print("[PASS] uname -a -> 5.2.0-FipSOs")

        cap = base.run_shell_cmd(qmp, "cat /sys/class/power_supply/BAT0/capacity", "100")
        assert "100" in cap
        st = base.run_shell_cmd(qmp, "cat /sys/class/power_supply/BAT0/status", "Full")
        assert "Full" in st
        prof = base.run_shell_cmd(qmp, "cat /sys/class/power_supply/BAT0/profile", "balanced")
        assert "balanced" in prof
        print("[PASS] /sys/class/power_supply/BAT0/{capacity,status,profile} initial read OK")

        base.run_shell_cmd(qmp, "echo powersave > /sys/class/power_supply/BAT0/profile", "fipsos:/#")
        prof2 = base.run_shell_cmd(qmp, "cat /sys/class/power_supply/BAT0/profile", "powersave")
        assert "powersave" in prof2
        print("[PASS] Switched power profile to powersave via sysfs")

        hwmon = base.run_shell_cmd(qmp, "cat /sys/fipsos/hwmon", "Bateria ACPI/EC (BAT0)")
        assert "FipSOs v5.2 Hardware Monitor" in hwmon
        assert "100% (Full, AC: ONLINE, Perfil: powersave)" in hwmon
        print("[PASS] /sys/fipsos/hwmon includes [PWR] BAT0 telemetry")

        start_gui = len(base.read_serial())
        qmp.send_text("gui\n")
        ok, _ = base.wait_for_serial("[desktop_ui.elf] FipSOs v5.2", timeout=15.0, offset=start_gui)
        assert ok, f"Desktop GUI v5.2 did not start:\n{base.read_serial()[start_gui:]}"
        time.sleep(1.0)

        shot2 = qmp.screendump("shot2_v52_desktop_battery")
        print(f"[PASS] Desktop v5.2 taskbar & Monitor battery UI -> {shot2}")

        qmp.send_key("q")
        base.wait_for_serial("fipsos@v5.2:/$", timeout=10.0, offset=start_gui)
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
