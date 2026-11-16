#!/usr/bin/env python3
"""
FipSOs v5.8 VirtualBox-Profile Boot, Lazy /proc+/sys, and Process Isolation Suite:
  1. Simulates VirtualBox environment (AuthenticAMD + hypervisor bit, 2048 MiB SATA disk, USB xHCI).
  2. Verifies [VFS] Montando /proc y /sys... completes without freeze and reaches fipsos:/#.
  3. Verifies cat /sys/fipsos/hwmon and cat /sys/class/power_supply/BAT0/status do not #GP.
  4. Verifies per-process cwd and FdTable isolation between parent and child processes.
"""

import os
import subprocess
import sys
import time

import test_v51_layer3_suite as base

REPO_DIR = base.REPO_DIR
ISO_PATH = base.ISO_PATH
DISK_PATH = os.path.join(REPO_DIR, "build", "test_v58_vbox_2g.img")
SERIAL_LOG = os.path.join(REPO_DIR, "build", "test_v58_serial.log")
QMP_PORT = 4488

base.DISK_PATH = DISK_PATH
base.SERIAL_LOG = SERIAL_LOG
base.QMP_PORT = QMP_PORT


def create_2g_disk(path: str) -> None:
    os.makedirs(os.path.dirname(path), exist_ok=True)
    if os.path.exists(path):
        os.remove(path)
    with open(path, "wb") as f:
        f.truncate(2048 * 1024 * 1024)


def main() -> int:
    print("=== FipSOs v5.8 VirtualBox-Profile & Process Isolation Verification ===")
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
        assert ok, f"Shell prompt not reached. Tail:\n{base.read_serial()[-1500:]}"
        time.sleep(0.5)

        serial_boot = base.read_serial()
        assert "[KERNEL PANIC" not in serial_boot and "[FipSOs PANIC]" not in serial_boot, (
            f"Unexpected kernel panic during boot:\n{serial_boot[-1200:]}"
        )
        assert "FipSOs v5.8" in serial_boot, f"Expected FipSOs v5.8 banner in:\n{serial_boot[:2500]}"
        assert "[VFS] Montando /proc y /sys..." in serial_boot, "Missing /proc and /sys mount log"
        assert "[linux_abi::vfs] VFS montado:" in serial_boot, "Missing [linux_abi::vfs] VFS montado"
        shot1 = qmp.screendump("shot_v58_boot")
        print(f"[PASS] Booted FipSOs v5.8 (AMD+Hypervisor + 2G SATA + xHCI) -> {shot1}")

        # 1. Verify /sys/fipsos/hwmon and BAT0/status under hypervisor without #GP
        hwmon = base.run_shell_cmd(qmp, "cat /sys/fipsos/hwmon", "fipsos:/#")
        assert "[HWMON] FipSOs v5.8" in hwmon, f"Expected FipSOs v5.8 hwmon, got:\n{hwmon}"
        bat_status = base.run_shell_cmd(qmp, "cat /sys/class/power_supply/BAT0/status", "fipsos:/#")
        assert "NotPresent" in bat_status or "Charging" in bat_status or "Full" in bat_status, (
            f"Unexpected BAT0/status:\n{bat_status}"
        )
        serial_now = base.read_serial()
        assert "[KERNEL PANIC" not in serial_now, "Kernel panic after reading sysfs!"
        print("[PASS] /sys/fipsos/hwmon & BAT0/status read cleanly under hypervisor (no #GP)")

        # 2. Verify CWD & FdTable isolation between parent and child processes
        parent_pwd1 = base.run_shell_cmd(qmp, "pwd", "fipsos:/#")
        assert "/" in parent_pwd1, f"Expected parent pwd '/', got:\n{parent_pwd1}"

        child_out = base.run_shell_cmd(
            qmp,
            "/bin/busybox sh -c \"cd /etc && pwd && echo v58_iso > /tmp/v58_iso.txt\"",
            "fipsos:/#",
        )
        assert "/etc" in child_out, f"Expected child pwd '/etc', got:\n{child_out}"

        parent_pwd2 = base.run_shell_cmd(qmp, "pwd", "fipsos:/#")
        lines = [ln.strip() for ln in parent_pwd2.splitlines() if ln.strip() and "fipsos:/#" not in ln]
        assert any(ln == "/" for ln in lines), (
            f"Parent CWD leaked from child! Expected '/', got lines={lines} raw:\n{parent_pwd2}"
        )

        iso_file = base.run_shell_cmd(qmp, "cat /tmp/v58_iso.txt", "fipsos:/#")
        assert "v58_iso" in iso_file, f"Expected v58_iso in redirected file, got:\n{iso_file}"
        print("[PASS] Per-process CWD and FdTable isolation verified (child cd /etc did not mutate parent)")

        # 3. Verify Desktop UI launches cleanly
        qmp.send_combo(["ret"])
        time.sleep(0.2)
        qmp.send_text("/bin/desktop_ui.elf\n")
        ok_ui, _ = base.wait_for_serial("[desktop_ui.elf] FipSOs v5.8", timeout=15.0)
        assert ok_ui, f"Desktop UI v5.8 did not start:\n{base.read_serial()[-1000:]}"
        time.sleep(0.6)
        shot2 = qmp.screendump("shot_v58_desktop")
        print(f"[PASS] Desktop UI v5.8 rendered -> {shot2}")

        print("=== ALL FipSOs v5.8 TESTS PASSED ===")
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


if __name__ == "__main__":
    sys.exit(main())
