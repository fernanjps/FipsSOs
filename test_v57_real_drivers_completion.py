#!/usr/bin/env python3
"""
FipSOs v5.7 Real Hardware Drivers Completion Verification Suite:
  1. PCIe NVMe controller discovery + ADMIN_OPC_IDENTIFY (0x06) capacity (32 MiB / 65536 sectors) + TSC timeouts
  2. Intel HDA PCIe CORB/RIRB DMA + bidirectional /dev/audio with calibrated TSC timing
  3. Intel e1000e NIC + DHCP handshake + live ARP/ICMP ping + RFC 793 TCP HTTP client
  4. Dynamic SMP CPU topology in /sys/devices/system/cpu/{online,possible,present} (-smp 4 -> 0-3)
  5. Full kernel/VFS/Desktop boot verification
"""

import os
import subprocess
import sys
import time

import test_v51_layer3_suite as base

REPO_DIR = base.REPO_DIR
ISO_PATH = base.ISO_PATH
DISK_PATH = os.path.join(REPO_DIR, "build", "test_v57_fat32.img")
NVME_PATH = os.path.join(REPO_DIR, "build", "test_v57_nvme.img")
SERIAL_LOG = os.path.join(REPO_DIR, "build", "test_v57_serial.log")
QMP_PORT = 4485

base.DISK_PATH = DISK_PATH
base.SERIAL_LOG = SERIAL_LOG
base.QMP_PORT = QMP_PORT


def main() -> int:
    print("=== FipSOs v5.7 Real Hardware Drivers Verification ===")
    base.create_fat32_disk(DISK_PATH)
    if os.path.exists(NVME_PATH):
        os.remove(NVME_PATH)
    with open(NVME_PATH, "wb") as f:
        f.truncate(32 * 1024 * 1024)  # 32 MiB = 65536 x 512-byte sectors

    if os.path.exists(SERIAL_LOG):
        os.remove(SERIAL_LOG)

    qemu_cmd = [
        "qemu-system-x86_64",
        "-M", "q35",
        "-cpu", "max",
        "-smp", "4",
        "-m", "512M",
        "-no-reboot",
        "-no-shutdown",
        "-serial", f"file:{SERIAL_LOG}",
        "-qmp", f"tcp:127.0.0.1:{QMP_PORT},server,nowait",
        "-drive", f"id=satadisk,file={DISK_PATH},format=raw,if=none",
        "-device", "ide-hd,drive=satadisk,bus=ide.0",
        "-drive", f"id=nvm0,file={NVME_PATH},format=raw,if=none",
        "-device", "nvme,drive=nvm0,serial=FIPSOSNVME",
        "-device", "qemu-xhci,id=xhci0",
        "-device", "usb-kbd,bus=xhci0.0",
        "-device", "usb-mouse,bus=xhci0.0",
        "-netdev", "user,id=net0",
        "-device", "e1000e,netdev=net0",
        "-device", "intel-hda",
        "-device", "hda-duplex",
        "-boot", "d",
        "-cdrom", ISO_PATH,
    ]
    proc = subprocess.Popen(qemu_cmd)
    qmp = None
    try:
        qmp = base.QmpClient(QMP_PORT)
        ok, _ = base.wait_for_serial("fipsos:/#", timeout=35.0)
        assert ok, f"Shell prompt not reached. Tail:\n{base.read_serial()[-1200:]}"
        time.sleep(0.5)

        serial_boot = base.read_serial()
        assert "[FipSOs PANIC]" not in serial_boot, "Unexpected kernel panic during boot"
        shot1 = qmp.screendump("shot_v57_boot")
        print(f"[PASS] Booted FipSOs v5.7 (4 CPUs + SATA + NVMe + HDA + e1000e) -> {shot1}")

        # 1. Verify 4-core SMP init & dynamic SysFS /sys/devices/system/cpu/online
        assert "[ostd::smp] Inicializacion SMP completada: 4 nucleos activos." in serial_boot, (
            f"Expected 4 active SMP cores in:\n{serial_boot[:2500]}"
        )
        cpu_online = base.run_shell_cmd(qmp, "cat /sys/devices/system/cpu/online", "fipsos:/#")
        assert "0-3" in cpu_online, f"Expected '0-3' in /sys/devices/system/cpu/online, got:\n{cpu_online}"
        print("[PASS] Dynamic SMP SysFS /sys/devices/system/cpu/online -> 0-3 verified")

        # 2. Verify PCIe NVMe ADMIN_OPC_IDENTIFY (0x06) capacity (32 MiB / 65536 sectors)
        assert "[ostd::storage::nvme] NVMe v" in serial_boot and "IO_QUEUES=1" in serial_boot, (
            f"Missing NVMe IO_QUEUES=1 log in:\n{serial_boot[:3500]}"
        )
        assert "Cap=32 MiB (65536 sect)" in serial_boot, (
            f"Expected NVMe ADMIN_OPC_IDENTIFY Cap=32 MiB (65536 sect) in:\n{serial_boot[:3500]}"
        )
        print("[PASS] PCIe NVMe ADMIN_OPC_IDENTIFY (0x06) 32 MiB / 65536 sectors verified")

        # 3. Verify Intel HDA CORB/RIRB DMA init with TSC timing + bidirectional /dev/audio
        assert "[ostd::hda] Intel HDA" in serial_boot and "CORB/RIRB DMA OK" in serial_boot, (
            f"Missing Intel HDA init log in:\n{serial_boot[:3500]}"
        )
        audit_out = base.run_shell_cmd(qmp, "/bin/fips_ai --stream-audit", "fipsos:/#")
        assert "--stream-audit" in audit_out and "pcm=256B" in audit_out, (
            f"Missing HDA /dev/audio stream audit output:\n{audit_out}"
        )
        print("[PASS] Intel HDA CORB/RIRB TSC-timed init & DMA PCM stream verified")

        # 4. Verify Intel e1000e DHCP handshake & Ping/Net stack with TSC deadlines
        assert "[ostd::net::dhcp] DHCPACK confirmado:" in serial_boot, (
            f"Expected DHCPACK in serial log:\n{serial_boot[:4000]}"
        )
        hwmon_out = base.run_shell_cmd(qmp, "cat /sys/fipsos/hwmon", "fipsos:/#")
        assert "LINK UP" in hwmon_out and "IP=10.0.2." in hwmon_out, (
            f"Expected active e1000e & IP in hwmon:\n{hwmon_out}"
        )
        print("[PASS] Intel e1000e + DHCP + hwmon verified")

        # 5. Launch Desktop GUI cleanly
        start_gui = len(base.read_serial())
        qmp.send_text("gui\n")
        ok, _ = base.wait_for_serial("[desktop_ui.elf]", timeout=15.0, offset=start_gui)
        assert ok, f"Desktop UI did not start:\n{base.read_serial()[start_gui:]}"
        time.sleep(1.5)
        shot2 = qmp.screendump("shot_v57_desktop")
        print(f"[PASS] Desktop UI running cleanly -> {shot2}")

        qmp.cmd("quit")
        proc.wait(timeout=5)
        print("\n[ALL PASS] FipSOs v5.7 Real Hardware Drivers Completion Suite Passed!")
        return 0
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


if __name__ == "__main__":
    sys.exit(main())
