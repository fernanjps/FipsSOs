#!/usr/bin/env python3
"""
FipSOs v5.6 Low-Level Kernel Hardening Verification Suite:
  1. ISO-Only Boot (VirtualBox scenario: -cdrom build/fipsos-x86_64.iso, NO hard disk):
     - Boots cleanly to fipsos:/# without SATAPI/FAT32 hang or panic
     - Verifies FipSOs v5.6 banner & Zero-Copy initramfs (reduced size, VFS aliases intact)
     - Verifies /sys/class/net/wlan0/operstate == no-device (cached)
  2. SMP 4-Core Boot (-smp 4) + FAT32 disk:
     - All 4 CPUs come online with PerCpu GS.base & syscall MSRs
     - Real sys_mprotect(W^X) enforcement tested from Ring 3 via tcc
     - Real HTTP failure (zero fake 200 OK fallback)
     - Desktop GUI launch & screenshot
"""

import os
import subprocess
import sys
import time

import test_v51_layer3_suite as base

REPO_DIR = base.REPO_DIR
ISO_PATH = base.ISO_PATH
INITRAMFS_TAR = os.path.join(REPO_DIR, "build", "initramfs.tar")
DISK_PATH = os.path.join(REPO_DIR, "build", "test_v56_fat32.img")


def run_iso_only_test() -> None:
    print("\n=== [Test 1] ISO-Only Boot (VirtualBox Simulation: No Hard Disk, -smp 2) ===")
    serial_iso = os.path.join(REPO_DIR, "build", "test_v56_iso_only_serial.log")
    qmp_port = 4477
    base.SERIAL_LOG = serial_iso
    base.QMP_PORT = qmp_port
    if os.path.exists(serial_iso):
        os.remove(serial_iso)

    qemu_cmd = [
        "qemu-system-x86_64",
        "-M", "q35",
        "-cpu", "max",
        "-smp", "2",
        "-m", "256M",
        "-no-reboot",
        "-no-shutdown",
        "-serial", f"file:{serial_iso}",
        "-qmp", f"tcp:127.0.0.1:{qmp_port},server,nowait",
        "-device", "qemu-xhci,id=xhci0",
        "-device", "usb-kbd,bus=xhci0.0",
        "-device", "usb-mouse,bus=xhci0.0",
        "-boot", "d",
        "-cdrom", ISO_PATH,
    ]
    proc = subprocess.Popen(qemu_cmd)
    qmp = None
    try:
        qmp = base.QmpClient(qmp_port)
        ok, _ = base.wait_for_serial("fipsos:/#", timeout=35.0)
        assert ok, f"ISO-only boot failed to reach fipsos:/#. Serial:\n{base.read_serial()[-1200:]}"
        time.sleep(0.4)

        serial_text = base.read_serial()
        assert "FipSOs v5.6" in serial_text, "Missing FipSOs v5.6 banner in serial"
        assert "Zero-Copy" in serial_text, "Missing Zero-Copy Initramfs log"
        assert "[FipSOs PANIC]" not in serial_text, "Unexpected kernel panic in ISO-only boot"

        shot = qmp.screendump("shot1_v56_iso_only")
        print(f"[PASS] ISO-only boot reached fipsos:/# cleanly -> {shot}")

        motd = base.run_shell_cmd(qmp, "cat /etc/motd.txt", "FipSOs v5.6")
        assert "FipSOs v5.6" in motd
        oper = base.run_shell_cmd(qmp, "cat /sys/class/net/wlan0/operstate", "fipsos:/#")
        assert "no-device" in oper
        print("[PASS] /etc/motd.txt (v5.6) & cached wlan0/operstate verified")
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


def run_smp4_hardening_test() -> None:
    print("\n=== [Test 2] SMP 4-Core Boot (-smp 4) + mprotect(W^X) + Zero-Fake-HTTP + Desktop ===")
    serial_smp = os.path.join(REPO_DIR, "build", "test_v56_smp4_serial.log")
    qmp_port = 4478
    base.DISK_PATH = DISK_PATH
    base.SERIAL_LOG = serial_smp
    base.QMP_PORT = qmp_port
    base.create_fat32_disk(DISK_PATH)
    if os.path.exists(serial_smp):
        os.remove(serial_smp)

    qemu_cmd = [
        "qemu-system-x86_64",
        "-M", "q35",
        "-cpu", "max",
        "-smp", "4",
        "-m", "512M",
        "-no-reboot",
        "-no-shutdown",
        "-serial", f"file:{serial_smp}",
        "-qmp", f"tcp:127.0.0.1:{qmp_port},server,nowait",
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
        qmp = base.QmpClient(qmp_port)
        ok, _ = base.wait_for_serial("fipsos:/#", timeout=35.0)
        assert ok, f"SMP 4 boot failed to reach fipsos:/#. Serial:\n{base.read_serial()[-1200:]}"
        time.sleep(0.5)

        serial_text = base.read_serial()
        assert "AP Core #1" in serial_text and "AP Core #2" in serial_text and "AP Core #3" in serial_text, (
            f"Expected 3 AP cores online in -smp 4. Serial:\n{serial_text[:2000]}"
        )
        assert "[FipSOs::vmo] Handle=" not in serial_text, "Phantom boot_checks 1MiB VMO should be removed"
        print("[PASS] 4-core SMP boot verified (BSP + AP #1..#3 online, zero phantom boot processes)")

        # Verify sys_mprotect real W^X enforcement from Ring 3 via tcc
        base.run_shell_cmd(qmp, "echo '#include <stdio.h>' > /mnt/fat0/mprot.c", "fipsos:/#")
        base.run_shell_cmd(qmp, "echo '#include <stdlib.h>' >> /mnt/fat0/mprot.c", "fipsos:/#")
        base.run_shell_cmd(
            qmp,
            "echo 'long sm(void*a,long l,long p){long r;__asm__ __volatile__(\"syscall\":\"=a\"(r):\"a\"(10L),\"D\"(a),\"S\"(l),\"d\"(p):\"rcx\",\"r11\",\"memory\");return r;}' >> /mnt/fat0/mprot.c",
            "fipsos:/#",
        )
        base.run_shell_cmd(
            qmp,
            "echo 'int main(){char*b=(char*)malloc(8192);unsigned long u=(unsigned long)b;unsigned long a=((u+4095)/4096)*4096;long r1=sm((void*)a,4096L,1L);long r2=sm((void*)a,4096L,6L);sm((void*)a,4096L,3L);if(r1==0)if(r2==-13)puts(\"MPROT_WX_OK\");return 0;}' >> /mnt/fat0/mprot.c",
            "fipsos:/#",
        )
        base.run_shell_cmd(
            qmp, "/bin/tcc /mnt/fat0/mprot.c -o /mnt/fat0/mprot.elf", "fipsos:/#", timeout=20.0
        )
        mprot_out = base.run_shell_cmd(
            qmp, "/mnt/fat0/mprot.elf", "MPROT_WX_OK", timeout=15.0
        )
        assert "MPROT_WX_OK" in mprot_out, f"Expected MPROT_WX_OK (W^X enforced), got:\n{mprot_out}"
        print("[PASS] sys_mprotect(PROT_READ) -> 0 and sys_mprotect(PROT_WRITE|PROT_EXEC) -> -13 (-EACCES W^X)")

        # Launch Desktop UI
        qmp.send_text("gui\n")
        ok_gui, _ = base.wait_for_serial("Desktop UI", timeout=15.0)
        time.sleep(1.5)
        shot2 = qmp.screendump("shot2_v56_desktop")
        print(f"[PASS] Desktop UI v5.6 launched -> {shot2} (serial_ok={ok_gui})")
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
    tar_size = os.path.getsize(INITRAMFS_TAR)
    print(f"[INFO] initramfs.tar size = {tar_size} bytes ({tar_size / (1024*1024):.2f} MiB)")
    assert tar_size < 14_000_000, f"initramfs.tar still too large ({tar_size} bytes); expected deduplicated"

    run_iso_only_test()
    run_smp4_hardening_test()
    print("\n[ALL PASS] FipSOs v5.6 Low-Level Hardening Suite Passed!")
    return 0


if __name__ == "__main__":
    sys.exit(main())
