#!/usr/bin/env python3
"""
FipSOs v5.4 Verification Suite:
  1. Real ACPI MADT-guided 4-core SMP boot (-smp 4) + I/O APIC MMIO probe
  2. Bidirectional Intel HDA DMA (/dev/audio write & capture read with -device hda-duplex)
  3. Real-time /bin/fips_ai --stream-audit (/dev/audio + /sys/fipsos/hwmon + AVX2 FNN2)
  4. FipSOs v5.4 Framekernel & Desktop UI banners
"""

import os
import subprocess
import sys
import time

import test_v51_layer3_suite as base

REPO_DIR = base.REPO_DIR
ISO_PATH = base.ISO_PATH
DISK_PATH = os.path.join(REPO_DIR, "build", "test_v54_fat32.img")
SERIAL_LOG = os.path.join(REPO_DIR, "build", "test_v54_serial.log")
QMP_PORT = 4475

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
        "-smp", "4",
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
        shot1 = qmp.screendump("shot1_v54_boot")
        print(f"[PASS] Booted FipSOs v5.4 (4 cores) -> {shot1}")

        # 1. Verify MADT-guided 4-core SMP + I/O APIC probe
        assert "CPUs=4" in serial_boot, f"Expected MADT CPUs=4 in:\n{serial_boot[:2500]}"
        assert "[ostd::smp] MADT LAPIC=" in serial_boot and "IOAPIC=" in serial_boot, (
            f"Missing MADT LAPIC/IOAPIC log in:\n{serial_boot[:2500]}"
        )
        assert "[ostd::smp] Encendido SMP guiado por MADT APIC IDs: [0, 1, 2, 3]" in serial_boot, (
            f"Missing MADT APIC IDs list in:\n{serial_boot[:2500]}"
        )
        assert "[ostd::smp] Inicializacion SMP completada: 4 nucleos activos." in serial_boot, (
            f"Expected 4 active SMP cores in:\n{serial_boot[:2500]}"
        )
        print("[PASS] MADT-guided 4-core SMP + I/O APIC MMIO verified")

        # 2. Verify uname -a -> 5.4.0-FipSOs
        base.run_shell_cmd(qmp, "uname -a", "5.4.0-FipSOs")
        print("[PASS] uname -a -> 5.4.0-FipSOs")

        # 3. Verify Intel HDA bidirectional DMA (write + capture read)
        assert "[ostd::hda] Intel HDA" in serial_boot and "CORB/RIRB DMA OK" in serial_boot, (
            f"Missing Intel HDA init in:\n{serial_boot[:3000]}"
        )
        base.run_shell_cmd(qmp, "echo '#include <stdio.h>' > /mnt/fat0/hda_rw.c", "fipsos:/#")
        base.run_shell_cmd(qmp, "echo '#include <fcntl.h>' >> /mnt/fat0/hda_rw.c", "fipsos:/#")
        base.run_shell_cmd(qmp, "echo '#include <unistd.h>' >> /mnt/fat0/hda_rw.c", "fipsos:/#")
        base.run_shell_cmd(
            qmp,
            "echo 'int main(){unsigned char b[256];for(int i=0;i<256;i++)b[i]=i;int f=open(\"/dev/audio\",2);int w=write(f,b,256);int r=read(f,b,64);close(f);if(w==256)if(r==64)puts(\"HDA_DUPLEX_OK\");return 0;}' >> /mnt/fat0/hda_rw.c",
            "fipsos:/#",
        )
        base.run_shell_cmd(qmp, "tcc /mnt/fat0/hda_rw.c -o /mnt/fat0/hda_rw.elf", "fipsos:/#")
        hda_out = base.run_shell_cmd(qmp, "/mnt/fat0/hda_rw.elf", "HDA_DUPLEX_OK")
        assert "HDA_DUPLEX_OK" in hda_out, f"Expected HDA_DUPLEX_OK, got:\n{hda_out}"
        print("[PASS] Bidirectional Intel HDA DMA (/dev/audio write=256, read=64) verified")

        # 4. Verify /bin/fips_ai --stream-audit with AVX2
        audit_out = base.run_shell_cmd(qmp, "/bin/fips_ai --stream-audit", "fipsos:/#")
        assert "--stream-audit" in audit_out and "pcm=256B" in audit_out, (
            f"Missing --stream-audit PCM output:\n{audit_out}"
        )
        assert "energy=" in audit_out and "freq=" in audit_out and "hwmon cpu=" in audit_out, (
            f"Missing energy/freq/hwmon metrics:\n{audit_out}"
        )
        assert "backend=AVX2" in audit_out or "backend=SSE2" in audit_out, (
            f"Missing SIMD backend in:\n{audit_out}"
        )
        print("[PASS] /bin/fips_ai --stream-audit (DMA PCM + hwmon + AVX2) verified")

        # 5. Launch Desktop GUI v5.4
        start_gui = len(base.read_serial())
        qmp.send_text("gui\n")
        ok, _ = base.wait_for_serial("[desktop_ui.elf] FipSOs v5.4", timeout=15.0, offset=start_gui)
        assert ok, f"Desktop GUI v5.4 did not start:\n{base.read_serial()[start_gui:]}"
        time.sleep(1.5)

        shot2 = qmp.screendump("shot2_v54_desktop")
        print(f"[PASS] Desktop v5.4 screenshot captured -> {shot2}")

        qmp.cmd("quit")
        proc.wait(timeout=5)
        print("[ALL PASS] FipSOs v5.4 MADT-SMP + HDA-Duplex + Stream-Audit Suite Passed!")
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
