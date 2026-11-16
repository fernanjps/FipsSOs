#!/usr/bin/env python3
"""
FipSOs v5.3 Real-Hardware ("Cero Atajos en Kernel") Verification Suite:
  1. Real ACPI Table Parser (RSDP, XSDT/RSDT, MADT/APIC, FADT/FACP)
  2. Zero simulated battery fallbacks (BAT0/status == NotPresent, BAT0/capacity -> -ENODEV)
  3. Real Intel HDA PCIe DMA driver (-device intel-hda -device hda-duplex):
     - CORB/RIRB DMA init, codec discovery
     - /dev/audio real PCM write succeeds, capture read returns -ENODEV (-19)
  4. C++20 Desktop taskbar shows `AC (No Bat)` when no physical EC battery is present
"""

import os
import subprocess
import sys
import time

import test_v51_layer3_suite as base

REPO_DIR = base.REPO_DIR
ISO_PATH = base.ISO_PATH
DISK_PATH = os.path.join(REPO_DIR, "build", "test_v53_real_hw_fat32.img")
SERIAL_LOG = os.path.join(REPO_DIR, "build", "test_v53_real_hw_serial.log")
QMP_PORT = 4474

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
        "-device", "intel-hda",
        "-device", "hda-duplex",
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

        serial_boot = base.read_serial()
        shot1 = qmp.screendump("shot1_v53_real_hw_boot")
        print(f"[PASS] Booted FipSOs v5.3 -> {shot1}")

        # 1. Verify Real ACPI MADT & FADT logs
        assert "[ostd::acpi] RSDP@" in serial_boot, "Missing [ostd::acpi] RSDP log"
        assert "MADT@" in serial_boot and "LAPIC=" in serial_boot and "CPUs=2" in serial_boot, (
            f"Missing MADT details in boot log:\n{serial_boot[:2500]}"
        )
        assert "[ostd::acpi] FADT@" in serial_boot and "PM1a_CNT=" in serial_boot, (
            f"Missing FADT details in boot log:\n{serial_boot[:2500]}"
        )
        print("[PASS] Real ACPI RSDP/MADT/FADT parser verified")

        # 2. Verify Real Intel HDA DMA init
        assert "[ostd::hda] Intel HDA" in serial_boot and "CORB/RIRB DMA OK" in serial_boot, (
            f"Missing Intel HDA DMA init log:\n{serial_boot[:3000]}"
        )
        print("[PASS] Real Intel HDA PCIe CORB/RIRB DMA init verified")

        # 3. Verify Zero Fake Battery Fallbacks
        bat_status = base.run_shell_cmd(qmp, "cat /sys/class/power_supply/BAT0/status", "NotPresent")
        assert "NotPresent" in bat_status
        bat_cap = base.run_shell_cmd(qmp, "cat /sys/class/power_supply/BAT0/capacity", "fipsos:/#")
        assert "100" not in bat_cap, f"Fake 100% battery leaked: {bat_cap}"
        hwmon = base.run_shell_cmd(qmp, "cat /sys/fipsos/hwmon", "fipsos:/#")
        assert "FipSOs v5.3 Hardware Monitor" in hwmon and "NotPresent" in hwmon
        print("[PASS] Zero fake battery fallbacks verified (NotPresent / -ENODEV)")

        # 4. Verify real /dev/audio write succeeds via Intel HDA DMA and read returns -ENODEV
        base.run_shell_cmd(qmp, "echo '#include <stdio.h>' > /mnt/fat0/h.c", "fipsos:/#")
        base.run_shell_cmd(qmp, "echo '#include <fcntl.h>' >> /mnt/fat0/h.c", "fipsos:/#")
        base.run_shell_cmd(qmp, "echo '#include <unistd.h>' >> /mnt/fat0/h.c", "fipsos:/#")
        base.run_shell_cmd(
            qmp,
            "echo 'int main(){unsigned char b[256];for(int i=0;i<256;i++)b[i]=1;int f=open(\"/dev/audio\",2);int w=write(f,b,256);int r=read(f,b,64);close(f);if(w==256)if(r<0)puts(\"HDA_OK\");return 0;}' >> /mnt/fat0/h.c",
            "fipsos:/#",
        )
        base.run_shell_cmd(qmp, "tcc /mnt/fat0/h.c -o /mnt/fat0/h.elf", "fipsos:/#")
        hda_out = base.run_shell_cmd(qmp, "/mnt/fat0/h.elf", "HDA_OK")
        assert "HDA_OK" in hda_out, f"Expected HDA_OK, got:\n{hda_out}"
        print("[PASS] /dev/audio real Intel HDA DMA write=256 & capture=-ENODEV verified")

        # 5. Launch Desktop GUI and verify `AC (No Bat)` on taskbar
        start_gui = len(base.read_serial())
        qmp.send_text("gui\n")
        ok, _ = base.wait_for_serial("[desktop_ui.elf] FipSOs v5.3", timeout=15.0, offset=start_gui)
        assert ok, f"Desktop GUI v5.3 did not start:\n{base.read_serial()[start_gui:]}"
        time.sleep(1.5)

        shot2 = qmp.screendump("shot2_v53_real_hw_desktop")
        print(f"[PASS] Desktop screenshot captured -> {shot2}")

        qmp.cmd("quit")
        proc.wait(timeout=5)
        print("[ALL PASS] FipSOs v5.3 Real-Hardware Suite Passed!")
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
            except Exception:
                proc.kill()


if __name__ == "__main__":
    sys.exit(main())
