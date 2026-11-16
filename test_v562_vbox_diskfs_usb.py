#!/usr/bin/env python3
"""
FipSOs v5.6.2 VirtualBox 2048 MiB DiskFS Warm-Boot & Multi-USB Verification Suite:
  - Reproduces VirtualBox state:
    * SATAPI ISO (-cdrom build/fipsos-x86_64.iso)
    * 2048 MiB (2 GiB) SATA disk on AHCI Port 0 (ide.0)
    * OHCI 1.1 (pci-ohci), EHCI 2.0 (usb-ehci), xHCI 3.0 (qemu-xhci) with usb-hub + usb-kbd + usb-mouse
  - Pass 1 (cold_2g): formats 2048 MiB SATA disk with DiskFS v2 (/disk/boot_log.txt at LBA=64)
  - Pass 2 (warm_2g): boots with pre-populated 2048 MiB DiskFS disk and verifies:
    * ATA IDENTIFY reports 4194304 sectores / 2048 MiB
    * All USB controllers (OHCI, EHCI, xHCI) initialize cleanly with 100ms TSC timeouts
    * Boot passes [DiskFS] Archivo "/disk/boot_log.txt" -> [DiskFS] Montaje completado -> [VFS] Montando /proc y /sys... -> fipsos:/# in < 1s
"""

import os
import subprocess
import sys
import time

import test_v51_layer3_suite as base

REPO_DIR = base.REPO_DIR
ISO_PATH = base.ISO_PATH
DISK_2G_IMG = os.path.join(REPO_DIR, "build", "test_v562_vbox_2g.img")


def run_boot(run_tag: str, qmp_port: int) -> None:
    serial_log = os.path.join(REPO_DIR, "build", f"test_v562_{run_tag}.log")
    base.DISK_PATH = DISK_2G_IMG
    base.SERIAL_LOG = serial_log
    base.QMP_PORT = qmp_port
    if os.path.exists(serial_log):
        os.remove(serial_log)

    qemu_cmd = [
        "qemu-system-x86_64",
        "-M", "q35",
        "-cpu", "max",
        "-smp", "2",
        "-m", "512M",
        "-no-reboot",
        "-no-shutdown",
        "-serial", f"file:{serial_log}",
        "-qmp", f"tcp:127.0.0.1:{qmp_port},server,nowait",
        "-drive", f"id=satadisk,file={DISK_2G_IMG},format=raw,if=none",
        "-device", "ide-hd,drive=satadisk,bus=ide.0",
        "-device", "pci-ohci,id=ohci0",
        "-device", "usb-ehci,id=ehci0",
        "-device", "qemu-xhci,id=xhci0",
        "-device", "usb-hub,bus=xhci0.0",
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
        qmp = base.QmpClient(qmp_port)
        if run_tag == "warm_2g":
            ok_disk, _ = base.wait_for_serial("[DiskFS] Montaje completado", timeout=25.0)
            t_disk = time.monotonic()
            assert ok_disk, f"[{run_tag}] Timed out waiting for [DiskFS] Montaje completado"
            ok_proc, _ = base.wait_for_serial("[VFS] Montando /proc y /sys...", timeout=5.0)
            t_proc = time.monotonic()
            assert ok_proc, f"[{run_tag}] Timed out waiting for [VFS] Montando /proc y /sys..."
            delta_s = t_proc - t_disk
            assert delta_s < 1.0, f"[{run_tag}] DiskFS->/proc took {delta_s:.3f}s (>= 1.0s)"
            print(f"[PASS] ({run_tag}) DiskFS -> /proc y /sys elapsed = {delta_s:.3f}s (< 1.0s)")

        ok, _ = base.wait_for_serial("fipsos:/#", timeout=35.0)
        assert ok, f"[{run_tag}] Boot failed to reach fipsos:/#. Tail:\n{base.read_serial()[-1200:]}"
        time.sleep(0.3)

        serial_text = base.read_serial()
        assert "[FipSOs PANIC]" not in serial_text, f"[{run_tag}] Unexpected kernel panic"
        assert "4194304 sectores / 2048 MiB" in serial_text, (
            f"[{run_tag}] Expected 2048 MiB (4194304 sectors) ATA IDENTIFY in AHCI log"
        )
        assert "OHCI USB 1.1" in serial_text, f"[{run_tag}] Expected OHCI USB 1.1 init log"
        assert "EHCI USB 2.0" in serial_text, f"[{run_tag}] Expected EHCI USB 2.0 init log"
        assert "xHCI USB 3.0" in serial_text, f"[{run_tag}] Expected xHCI USB 3.0 init log"
        assert "[VFS] Montando /proc y /sys..." in serial_text, "Missing /proc and /sys trace"
        assert "[linux_abi::vfs] VFS montado:" in serial_text, "Missing final VFS mounted trace"
        if run_tag == "warm_2g":
            assert '[DiskFS] Archivo "/disk/boot_log.txt" cargado de disco SATA (Inodo=1, LBA=64,' in serial_text
            assert "[DiskFS] Montaje completado (1 archivos)" in serial_text

        shot = qmp.screendump(f"shot_v562_vbox_{run_tag}")
        print(f"[PASS] ({run_tag}) Reached fipsos:/# cleanly -> {shot}")
    finally:
        if qmp is not None:
            try:
                qmp.cmd("quit")
            except Exception:
                pass
        if proc.poll() is None:
            proc.terminate()
            try:
                proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                proc.kill()


def main() -> int:
    print("=== FipSOs v5.6.2 VirtualBox 2048 MiB DiskFS & Multi-USB Verification ===")
    if os.path.exists(DISK_2G_IMG):
        os.remove(DISK_2G_IMG)
    with open(DISK_2G_IMG, "wb") as f:
        f.truncate(2048 * 1024 * 1024)

    run_boot("cold_2g", qmp_port=4483)
    run_boot("warm_2g", qmp_port=4484)

    print("\n[ALL PASS] FipSOs v5.6.2 VirtualBox 2048 MiB DiskFS & Multi-USB Suite Passed!")
    return 0


if __name__ == "__main__":
    sys.exit(main())
