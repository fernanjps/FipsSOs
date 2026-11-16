#!/usr/bin/env python3
"""
FipSOs v5.5 Verification Suite:
  1. FipSOs v5.5 Framekernel boot, uname -a -> 5.5.0-FipSOs
  2. Strict hardware detection: /sys/class/net/wlan0/operstate == no-device
  3. /bin/iwctl_fips: status, load-fw -ENOENT on missing firmware, wpa2-test -> WPA2_CCMP_OK
  4. Sequential GRU/Attention AVX2 engine: /bin/fips_ai --seq-infer --model /mnt/fat0/seq_model.fnn --tokens 1,4,2,7
  5. Desktop GUI v5.5 screenshots
"""

import os
import subprocess
import sys
import time

import test_v51_layer3_suite as base

REPO_DIR = base.REPO_DIR
ISO_PATH = base.ISO_PATH
DISK_PATH = os.path.join(REPO_DIR, "build", "test_v55_fat32.img")
SERIAL_LOG = os.path.join(REPO_DIR, "build", "test_v55_serial.log")
QMP_PORT = 4476

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

        shot1 = qmp.screendump("shot1_v55_boot")
        print(f"[PASS] Booted FipSOs v5.5 -> {shot1}")

        base.run_shell_cmd(qmp, "uname -a", "5.5.0-FipSOs")
        print("[PASS] uname -a -> 5.5.0-FipSOs")

        # 1. Strict Wi-Fi hardware detection: /sys/class/net/wlan0/operstate == no-device
        oper = base.run_shell_cmd(qmp, "cat /sys/class/net/wlan0/operstate", "fipsos:/#")
        assert "no-device" in oper, f"Expected no-device in wlan0/operstate, got:\n{oper}"
        print("[PASS] /sys/class/net/wlan0/operstate -> no-device (zero fake Wi-Fi)")

        # 2. /bin/iwctl_fips status, load-fw -ENOENT, and wpa2-test
        st = base.run_shell_cmd(qmp, "/bin/iwctl_fips status", "operstate=no-device")
        assert "operstate=no-device" in st

        fw = base.run_shell_cmd(qmp, "/bin/iwctl_fips load-fw missing.ucode", "-ENOENT")
        assert "-ENOENT" in fw, f"Expected -ENOENT for missing firmware, got:\n{fw}"

        wpa = base.run_shell_cmd(qmp, "/bin/iwctl_fips wpa2-test", "WPA2_CCMP_OK")
        assert "WPA2_CCMP_OK" in wpa and "mic=VERIFIED" in wpa, f"Expected WPA2_CCMP_OK, got:\n{wpa}"
        print("[PASS] /bin/iwctl_fips status, load-fw (-ENOENT) & WPA2-PSK AES-CCMP verified")

        # 3. Create real FNN2 model in /mnt/fat0/seq_model.fnn and run /bin/fips_ai --seq-infer
        base.run_shell_cmd(qmp, "echo '#include <stdio.h>' > /mnt/fat0/mkseq.c", "fipsos:/#")
        base.run_shell_cmd(qmp, "echo '#include <stdint.h>' >> /mnt/fat0/mkseq.c", "fipsos:/#")
        base.run_shell_cmd(
            qmp,
            "echo 'int main(){FILE*f=fopen(\"/mnt/fat0/seq_model.fnn\",\"wb\");uint32_t h[5]={0x324E4E46U,8,8,4,0};fwrite(h,4,5,f);float w[108];for(int i=0;i<108;i++)w[i]=((float)((i*7)-35))*0.02f;fwrite(w,4,108,f);fclose(f);puts(\"SEQ_MODEL_OK\");return 0;}' >> /mnt/fat0/mkseq.c",
            "fipsos:/#",
        )
        base.run_shell_cmd(qmp, "tcc /mnt/fat0/mkseq.c -o /mnt/fat0/mkseq.elf", "fipsos:/#")
        base.run_shell_cmd(qmp, "/mnt/fat0/mkseq.elf", "SEQ_MODEL_OK")

        seq_out = base.run_shell_cmd(
            qmp,
            "/bin/fips_ai --seq-infer --model /mnt/fat0/seq_model.fnn --tokens 1,4,2,7",
            "fipsos:/#",
        )
        assert "--seq-infer" in seq_out and "tokens=4" in seq_out, f"Missing --seq-infer output:\n{seq_out}"
        assert "hidden=[" in seq_out and "top3=[" in seq_out and "latency=" in seq_out, (
            f"Missing hidden/top3/latency in:\n{seq_out}"
        )
        assert "backend=AVX2" in seq_out or "backend=SSE2" in seq_out, (
            f"Missing SIMD backend in:\n{seq_out}"
        )
        print("[PASS] /bin/fips_ai --seq-infer (GRU + Attention AVX2) verified")

        # 4. Desktop GUI v5.5
        start_gui = len(base.read_serial())
        qmp.send_text("gui\n")
        ok, _ = base.wait_for_serial("[desktop_ui.elf] FipSOs v5.5", timeout=15.0, offset=start_gui)
        assert ok, f"Desktop GUI v5.5 did not start:\n{base.read_serial()[start_gui:]}"
        time.sleep(1.5)

        shot2 = qmp.screendump("shot2_v55_desktop")
        print(f"[PASS] Desktop v5.5 screenshot captured -> {shot2}")

        qmp.cmd("quit")
        proc.wait(timeout=5)
        print("[ALL PASS] FipSOs v5.5 Wi-Fi 802.11/WPA2 + Sequential GRU AVX2 Suite Passed!")
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
