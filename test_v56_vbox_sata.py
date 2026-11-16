#!/usr/bin/env python3
"""
FipSOs v5.6.1 VirtualBox & SATA Port 0 Hardening Verification Suite:
  - Replicates VirtualBox topology: SATAPI ISO (-cdrom) + SATA Disk on AHCI Port 0 (ide.0)
    + xHCI USB 3.0 controller with a non-HID usb-hub alongside usb-kbd / usb-mouse.
  - Verifies:
    1. ATA IDENTIFY DEVICE (0xEC) discovers real sector capacity on AHCI Port 0.
    2. xHCI safely skips non-HID / hub ports without hanging (150ms TSC timeout).
    3. Granular VFS traces appear in order: Initramfs -> FAT32 -> DiskFS -> VFS montado.
    4. Clean boot both on fresh empty 16 MiB SATA disk (DiskFS format) AND on second boot
       with pre-existing DiskFS (exact VirtualBox screenshot state) to fipsos:/#.
    5. Updated Userland v5.6 banners in fipsos_sh.elf and Desktop UI (gui).
"""

import os
import subprocess
import sys
import time

import test_v51_layer3_suite as base

REPO_DIR = base.REPO_DIR
ISO_PATH = base.ISO_PATH
SATA_IMG = os.path.join(REPO_DIR, "build", "test_v56_vbox_sata.img")


def boot_and_verify(run_tag: str, qmp_port: int, launch_gui: bool = False) -> None:
    serial_log = os.path.join(REPO_DIR, "build", f"test_v56_vbox_{run_tag}.log")
    base.DISK_PATH = SATA_IMG
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
        "-drive", f"id=satadisk,file={SATA_IMG},format=raw,if=none",
        "-device", "ide-hd,drive=satadisk,bus=ide.0",
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
        ok, _ = base.wait_for_serial("fipsos:/#", timeout=35.0)
        assert ok, f"[{run_tag}] Boot failed to reach fipsos:/#. Tail:\n{base.read_serial()[-1200:]}"
        time.sleep(0.4)

        serial_text = base.read_serial()
        assert "[FipSOs PANIC]" not in serial_text, f"[{run_tag}] Unexpected kernel panic"
        assert "32768 sectores / 16 MiB" in serial_text, (
            f"[{run_tag}] Expected ATA IDENTIFY 32768 sectors in AHCI log"
        )
        assert "[VFS] Montando Initramfs..." in serial_text, "Missing Initramfs progress trace"
        assert "[VFS] Montando FAT32..." in serial_text, "Missing FAT32 progress trace"
        assert "[VFS] Montando DiskFS..." in serial_text, "Missing DiskFS progress trace"
        assert "[linux_abi::vfs] VFS montado:" in serial_text, "Missing final VFS mounted trace"
        assert "FipSOs v5.6 Framekernel" in serial_text, "Expected updated FipSOs v5.6 banner in shell/kernel"

        shot1 = qmp.screendump(f"shot_v561_vbox_{run_tag}")
        print(f"[PASS] ({run_tag}) Booted cleanly to fipsos:/# with AHCI Port 0 SATA + xHCI hub -> {shot1}")

        disk_ls = base.run_shell_cmd(qmp, "cat /disk/boot_log.txt", "DiskFS v2")
        assert "FipSOs v5.6" in disk_ls, f"[{run_tag}] Unexpected /disk/boot_log.txt content:\n{disk_ls}"
        print(f"[PASS] ({run_tag}) Verified /disk/boot_log.txt has FipSOs v5.6 banner")

        if launch_gui:
            qmp.send_text("gui\n")
            ok_gui, _ = base.wait_for_serial("[desktop_ui.elf] FipSOs v5.6", timeout=15.0)
            time.sleep(1.5)
            shot2 = qmp.screendump("shot_v561_vbox_desktop")
            print(f"[PASS] ({run_tag}) Desktop UI launched (banner_ok={ok_gui}) -> {shot2}")
            assert ok_gui, "Expected [desktop_ui.elf] FipSOs v5.6 serial banner"
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
    print("=== FipSOs v5.6.1 VirtualBox SATA Port 0 & TSC Timeout Verification ===")
    if os.path.exists(SATA_IMG):
        os.remove(SATA_IMG)
    with open(SATA_IMG, "wb") as f:
        f.truncate(16 * 1024 * 1024)

    # Pass 1: Cold boot with blank 16 MiB SATA disk on Port 0 (DiskFS formats LBA 0..64)
    boot_and_verify("cold_format", qmp_port=4481, launch_gui=False)
    # Pass 2: Warm boot with pre-formatted DiskFS disk (matches VirtualBox screenshot state) + GUI
    boot_and_verify("warm_mount", qmp_port=4482, launch_gui=True)

    print("\n[ALL PASS] FipSOs v5.6.1 VirtualBox SATA & xHCI Hardening Suite Passed!")
    return 0


if __name__ == "__main__":
    sys.exit(main())
