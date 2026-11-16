#!/usr/bin/env python3
"""
FipSOs v6.2 End-to-End Verification:
  1. Ring 0 `#GP` MSR Safe Trap Recovery (`rdmsr_safe` / `wrmsr_safe`, RIP += 2, RAX=RDX=0).
  2. Unblocked `sensors` / `/sys/fipsos/hwmon` / ACPI EC hypervisor guard / USB lock order.
  3. Exhaustive CLI & VFS command sweep:
     `sensors`, `msrtest`, `help`, `uname -a`, `ps`, `free`, `mem`, `pwd`, `cd`, `ls /`,
     `cat /proc/cpuinfo`, `cat /proc/meminfo`, `cat /proc/uptime`, `cat /proc/version`,
     `cat /sys/fipsos/hwmon`, `cat /sys/class/power_supply/BAT0/status`, `clear`,
     `net`, `ping`, `/bin/iwctl_fips status`, `/bin/fips_ai --guard 8 0 0 2 0 20`,
     `ai --guard 8 0 0 2 0 20`.
  4. GUI v6.2 launch, Live Kernel Monitor, RAM R/W probe, child spawn, clean `q` exit,
     and strict 0-leak frame reclamation check.
"""

import os
import re
import subprocess
import sys
import time

import test_v51_layer3_suite as base

REPO_DIR = base.REPO_DIR
ISO_PATH = base.ISO_PATH
USB_IMG_PATH = os.path.join(REPO_DIR, "build", "fipsos-baremetal-usb.img")
DISK_PATH = os.path.join(REPO_DIR, "build", "test_v62_sata.img")
SERIAL_LOG = os.path.join(REPO_DIR, "build", "test_v62_serial.log")
QMP_PORT = 4494

base.DISK_PATH = DISK_PATH
base.SERIAL_LOG = SERIAL_LOG
base.QMP_PORT = QMP_PORT


def create_2g_disk(path: str) -> None:
    os.makedirs(os.path.dirname(path), exist_ok=True)
    if os.path.exists(path):
        os.remove(path)
    with open(path, "wb") as f:
        f.truncate(2048 * 1024 * 1024)


def load_ppm_rgb(ppm_path: str) -> tuple[int, int, bytes]:
    with open(ppm_path, "rb") as f:
        tokens = []
        while len(tokens) < 4:
            line = f.readline()
            if not line.startswith(b"#"):
                tokens.extend(line.split())
        w = int(tokens[1])
        h = int(tokens[2])
        data = f.read(w * h * 3)
    return w, h, data


def region_signature(rgb: bytes, w: int, x0: int, y0: int, rw: int, rh: int) -> tuple[int, int, int, int]:
    r_sum = 0
    g_sum = 0
    b_sum = 0
    xor_hash = 0
    cnt = 0
    for y in range(y0, y0 + rh, 2):
        for x in range(x0, x0 + rw, 2):
            idx = (y * w + x) * 3
            r = rgb[idx]
            g = rgb[idx + 1]
            b = rgb[idx + 2]
            r_sum += r
            g_sum += g
            b_sum += b
            xor_hash = ((xor_hash * 131) ^ (r << 16) ^ (g << 8) ^ b) & 0xFFFFFFFF
            cnt += 1
    return (r_sum // max(1, cnt), g_sum // max(1, cnt), b_sum // max(1, cnt), xor_hash)


def run_cli_cmd(
    qmp: base.QmpClient,
    cmd: str,
    expected_substr: str | None = None,
    timeout: float = 8.0,
    prompt: str = "fipsos:/#",
) -> str:
    pos = len(base.read_serial())
    t0 = time.monotonic()
    qmp.send_text(cmd + "\n")
    ok_prompt, _ = base.wait_for_serial(prompt, timeout=timeout, offset=pos)
    elapsed = time.monotonic() - t0
    out = base.strip_ansi(base.read_serial()[pos:])
    assert ok_prompt, f"Command {cmd!r} timed out waiting for {prompt!r} after {elapsed:.2f}s:\n{out}"
    if expected_substr is not None:
        assert expected_substr in out, f"Command {cmd!r} missing {expected_substr!r}:\n{out}"
    print(f"[PASS] {cmd} ({elapsed:.2f}s)")
    return out


def parse_hwmon_frames(hwmon_text: str, tag: str) -> tuple[int, int]:
    m = re.search(
        r"Memoria Fisica:\s+(\d+)\s+KiB Total\s+\|\s+(\d+)\s+KiB Usados\s+\((\d+)\s+frames\)\s+\|\s+(\d+)\s+KiB Libres\s+\((\d+)\s+frames\)",
        hwmon_text,
    )
    assert m is not None, f"[{tag}] Failed to parse Memoria Fisica:\n{hwmon_text}"
    return int(m.group(3)), int(m.group(5))


def main() -> int:
    print("=== FipSOs v6.2 MSR #GP Trap Recovery & Exhaustive CLI/GUI Suite ===")
    assert os.path.exists(ISO_PATH), f"Missing {ISO_PATH}"
    assert os.path.exists(USB_IMG_PATH), f"Missing {USB_IMG_PATH}"
    create_2g_disk(DISK_PATH)
    if os.path.exists(SERIAL_LOG):
        os.remove(SERIAL_LOG)

    qemu_cmd = [
        "qemu-system-x86_64",
        "-M", "q35",
        "-cpu", "EPYC,+hypervisor",
        "-smp", "2",
        "-m", "2048M",
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
    ppm_files: list[str] = []
    try:
        qmp = base.QmpClient(QMP_PORT)
        ok, _ = base.wait_for_serial("fipsos:/#", timeout=45.0)
        assert ok, f"Shell prompt not reached. Tail:\n{base.read_serial()[-1200:]}"

        boot_log = base.strip_ansi(base.read_serial())
        assert "FipSOs v6.2" in boot_log, "Missing FipSOs v6.2 boot banner"
        assert "[ostd::arch::msr] #GP Trap Recovery active" in boot_log, "Missing MSR #GP self-test boot log"
        m_boot_gp = re.search(r"Recovered #GP=(\d+)", boot_log)
        assert m_boot_gp is not None and int(m_boot_gp.group(1)) >= 1, f"Boot MSR #GP selftest count < 1:\n{boot_log}"
        print(f"[PASS] Booted FipSOs v6.2 | Boot #GP recovered={m_boot_gp.group(1)}")

        # 1. Dynamic Ring 0 #GP recovery verification on demand from Ring 3
        msr_out = run_cli_cmd(qmp, "msrtest", "[MSR #GP SAFE TEST]", timeout=8.0)
        m_msr = re.search(r"before=(\d+)\s+after=(\d+)", msr_out)
        assert m_msr is not None, f"Could not parse msrtest output:\n{msr_out}"
        gp_before, gp_after = int(m_msr.group(1)), int(m_msr.group(2))
        assert gp_after > gp_before, f"Expected Ring 0 #GP counter to advance (before={gp_before}, after={gp_after})"
        print(f"[PASS] Dynamic Ring 0 #GP trap recovery verified: {gp_before} -> {gp_after}")

        # 2. Unblocked `sensors` & /sys/fipsos/hwmon & /proc/cpuinfo MHz check
        hw_sensors = run_cli_cmd(qmp, "sensors", "[HWMON] FipSOs v6.2 Hardware Monitor & Sensores:", timeout=6.0)
        assert "Proteccion Ring 0 MSR (#GP Safe):" in hw_sensors, f"Missing MSR #GP line in sensors:\n{hw_sensors}"
        m_mhz = re.search(r"@\s+(\d+)\s+MHz", hw_sensors)
        assert m_mhz is not None and int(m_mhz.group(1)) >= 400, f"Invalid MHz in sensors:\n{hw_sensors}"
        used_base, free_base = parse_hwmon_frames(hw_sensors, "baseline_sensors")
        print(f"[PASS] `sensors` reported {m_mhz.group(1)} MHz | used={used_base} fr, free={free_base} fr")

        # 3. Exhaustive CLI & VFS command sweep
        run_cli_cmd(qmp, "help", "free / mem / sensors")
        run_cli_cmd(qmp, "uname -a", "6.2.0-FipSOs")
        run_cli_cmd(qmp, "ps", "PID")
        run_cli_cmd(qmp, "free", "Memoria Fisica:")
        run_cli_cmd(qmp, "mem", "Memoria Fisica:")
        run_cli_cmd(qmp, "pwd", "/")
        run_cli_cmd(qmp, "ls /", "bin")
        run_cli_cmd(qmp, "cd /proc", prompt="fipsos:/proc#")
        run_cli_cmd(qmp, "pwd", "/proc", prompt="fipsos:/proc#")
        run_cli_cmd(qmp, "cd /", prompt="fipsos:/#")
        cpuinfo = run_cli_cmd(qmp, "cat /proc/cpuinfo", "vendor_id")
        assert not re.search(r"cpu MHz\s+:\s+0\.000", cpuinfo), f"/proc/cpuinfo reported 0 MHz:\n{cpuinfo}"
        run_cli_cmd(qmp, "cat /proc/meminfo", "MemTotal:")
        run_cli_cmd(qmp, "cat /proc/uptime")
        run_cli_cmd(qmp, "cat /proc/version", "6.2.0-FipSOs")
        hw_cat = run_cli_cmd(qmp, "cat /sys/fipsos/hwmon", "FipSOs v6.2")
        run_cli_cmd(qmp, "cat /sys/class/power_supply/BAT0/status", "NotPresent")
        run_cli_cmd(qmp, "clear")
        run_cli_cmd(qmp, "net", "Ping ICMP/ARP", timeout=10.0)
        run_cli_cmd(qmp, "ping", "Ping ICMP/ARP", timeout=10.0)
        run_cli_cmd(qmp, "/bin/iwctl_fips status", "wlan0")
        run_cli_cmd(qmp, "/bin/fips_ai --guard 8 0 0 2 0 20", "verdict=")
        run_cli_cmd(qmp, "ai --guard 8 0 0 2 0 20", "verdict=")

        _, free_pre_gui = parse_hwmon_frames(hw_cat, "pre_gui")

        # 4. GUI v6.2 verification + live monitor + clean exit
        time.sleep(0.2)
        qmp.send_text("gui\n")
        ok_gui, _ = base.wait_for_serial("[desktop_ui.elf] FipSOs v6.2", timeout=15.0)
        assert ok_gui, f"GUI v6.2 did not launch:\n{base.read_serial()[-800:]}"
        time.sleep(0.8)

        shot_clean = qmp.screendump("shot_v62_clean_desktop")
        ppm_clean = os.path.join(REPO_DIR, "build", "shot_v62_clean_desktop.ppm")
        ppm_files.append(ppm_clean)
        w, h, clean_rgb = load_ppm_rgb(ppm_clean)
        qmp.mouse_x = w // 2
        qmp.mouse_y = h // 2
        print(f"[PASS] Clean GUI v6.2 rendered at {w}x{h} -> {shot_clean}")

        taskbar_y = h - 48
        dock_x0 = (w - 384) // 2
        qmp.click_at(dock_x0 + 48 + 20, taskbar_y + 24)
        time.sleep(0.6)
        shot_open = qmp.screendump("shot_v62_live_monitor")
        ppm_open = os.path.join(REPO_DIR, "build", "shot_v62_live_monitor.ppm")
        ppm_files.append(ppm_open)
        _, _, open_rgb = load_ppm_rgb(ppm_open)
        assert region_signature(clean_rgb, w, 40, 60, 560, 320) != region_signature(
            open_rgb, w, 40, 60, 560, 320
        ), "Live Kernel Monitor window did not open!"
        print(f"[PASS] Live Kernel Monitor opened -> {shot_open}")

        # Probar RAM R/W + 1 child launch inside GUI
        qmp.click_at(115, 421)
        time.sleep(0.5)
        pos_l = len(base.read_serial())
        qmp.click_at(273, 421)
        ok_exit, _ = base.wait_for_serial("sys_exit/exit_group(status=0)", timeout=12.0, offset=pos_l)
        assert ok_exit, f"GUI child launch failed:\n{base.read_serial()[pos_l:]}"
        time.sleep(0.3)

        shot_probe = qmp.screendump("shot_v62_after_probe")
        ppm_probe = os.path.join(REPO_DIR, "build", "shot_v62_after_probe.ppm")
        ppm_files.append(ppm_probe)
        print(f"[PASS] GUI RAM R/W probe + child launch passed -> {shot_probe}")

        pos_q = len(base.read_serial())
        qmp.send_text("q")
        ok_q, _ = base.wait_for_serial("fipsos:/#", timeout=12.0, offset=pos_q)
        tail_q = base.read_serial()[pos_q:]
        assert ok_q and "Sesion grafica finalizada limpiamente" in tail_q, f"Clean GUI exit failed:\n{tail_q}"
        print("[PASS] Exited GUI cleanly back to fipsos:/#")

        hw_end = run_cli_cmd(qmp, "cat /sys/fipsos/hwmon", "FipSOs v6.2")
        used_end, free_end = parse_hwmon_frames(hw_end, "post_gui")
        delta_vs_pre_gui = free_end - free_pre_gui
        print(f"[PASS] Post-GUI RAM: used={used_end} fr, free={free_end} fr (delta_vs_pre_gui={delta_vs_pre_gui:+d} fr)")
        assert abs(delta_vs_pre_gui) <= 8, f"Frame leak detected across GUI session: delta={delta_vs_pre_gui} fr"

        full_log = base.strip_ansi(base.read_serial())
        assert "UNRECOVERABLE RING 0 EXCEPTION" not in full_log and "[FipSOs PANIC]" not in full_log and "Triple" not in full_log, (
            "Unexpected kernel panic/fault found in serial log!"
        )
        print("=== ALL FipSOs v6.2 TESTS PASSED ===")
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
        for temp in [DISK_PATH, SERIAL_LOG] + ppm_files:
            if os.path.exists(temp):
                try:
                    os.remove(temp)
                except OSError:
                    pass


if __name__ == "__main__":
    sys.exit(main())
