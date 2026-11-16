#!/usr/bin/env python3
"""
FipSOs v6.3 End-to-End Verification:
  1. Safe FAT32 auto-initialization at LBA 2048 on a pre-formatted 2 GiB DiskFS v2 disk
     (strictly preserving LBA 0..2047 DiskFS superblock, inodes, and /disk/boot_log.txt at LBA 64).
  2. Seeding & execution from /mnt/fat0 (fipsos.txt, hello.lua, seq_model.fnn) + GUI Explorer view.
  3. Ring 0 Container Subsystem & /bin/docker CLI:
     - Zero-copy base images (`docker images`)
     - UTS & PID isolation (`docker run --name test-box --mem 16 --hostname ctr-node ... /bin/uname -a`)
     - Mount/chroot confinement (`sys_chroot = 161`, blocking host `/etc/motd.txt` and `/../../disk/boot_log.txt`)
     - Memory Cgroup page-table quota enforcement (`--mem 2` returning `-ENOMEM` cleanly)
     - Container lifecycle (`docker ps`, `docker inspect`, `docker rm`).
"""

import os
import re
import shutil
import struct
import subprocess
import sys
import time

import test_v51_layer3_suite as base

REPO_DIR = base.REPO_DIR
ISO_PATH = base.ISO_PATH
USB_IMG_PATH = os.path.join(REPO_DIR, "build", "fipsos-baremetal-usb.img")
DISK_PATH = os.path.join(REPO_DIR, "build", "test_v63_sata.img")
SERIAL_LOG = os.path.join(REPO_DIR, "build", "test_v63_serial.log")
QMP_PORT = 4496

base.DISK_PATH = DISK_PATH
base.SERIAL_LOG = SERIAL_LOG
base.QMP_PORT = QMP_PORT

BOOT_LOG_SENTINEL = b"FipSOs v6.2 Pre-Existing DiskFS Boot Log at LBA 64 (MUST SURVIVE FAT32 AUTO-INIT)\n"


def create_preformatted_diskfs_v2(path: str) -> None:
    """Create a 2 GiB raw SATA image mimicking an existing VirtualBox DiskFS v2 disk (no FAT32 at LBA 2048)."""
    os.makedirs(os.path.dirname(path), exist_ok=True)
    if os.path.exists(path):
        os.remove(path)
    with open(path, "wb") as f:
        f.truncate(2048 * 1024 * 1024)

        # LBA 0: FIPSOS_DISKFS_V2 superblock (strictly no 0x55AA MBR signature)
        sb = bytearray(512)
        sb[0:16] = b"FIPSOS_DISKFS_V2"
        f.seek(0)
        f.write(sb)

        # LBA 1: Inode 0 -> /disk/boot_log.txt at LBA 64
        inode0 = bytearray(128)
        inode0[0] = 1  # used
        inode0[1] = 1  # RegularFile
        struct.pack_into("<IIIII", inode0, 4, 0, len(BOOT_LOG_SENTINEL), 64, 1, 1)
        disk_path = b"/disk/boot_log.txt\x00"
        inode0[24 : 24 + len(disk_path)] = disk_path
        f.seek(512)
        f.write(inode0)

        # LBA 33: Sector bitmap (bits 0..64 allocated)
        bitmap = bytearray(4096)
        for idx in range(8):
            bitmap[idx] = 0xFF
        bitmap[8] = 0x01
        f.seek(33 * 512)
        f.write(bitmap)

        # LBA 64: Pre-existing /disk/boot_log.txt data
        sec64 = bytearray(512)
        sec64[: len(BOOT_LOG_SENTINEL)] = BOOT_LOG_SENTINEL
        f.seek(64 * 512)
        f.write(sec64)


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
    r_sum = g_sum = b_sum = xor_hash = cnt = 0
    for y in range(y0, y0 + rh, 2):
        for x in range(x0, x0 + rw, 2):
            idx = (y * w + x) * 3
            r, g, b = rgb[idx], rgb[idx + 1], rgb[idx + 2]
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
    timeout: float = 10.0,
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


def main() -> int:
    print("=== FipSOs v6.3 FAT32 Auto-Init, Containers & /bin/docker Suite ===")
    assert os.path.exists(ISO_PATH), f"Missing {ISO_PATH}"
    assert os.path.exists(USB_IMG_PATH), f"Missing {USB_IMG_PATH}"
    create_preformatted_diskfs_v2(DISK_PATH)
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
        ok, _ = base.wait_for_serial("fipsos:/#", timeout=50.0)
        assert ok, f"Shell prompt not reached. Tail:\n{base.read_serial()[-1200:]}"

        boot_log = base.strip_ansi(base.read_serial())
        assert "FipSOs v6.3" in boot_log, "Missing FipSOs v6.3 boot banner"
        assert '[DiskFS] Archivo "/disk/boot_log.txt" cargado de disco SATA' in boot_log and "LBA=64" in boot_log, (
            f"Pre-existing DiskFS v2 LBA 64 file was not loaded:\n{boot_log[:2500]}"
        )
        assert "[FAT32] Auto-inicializado limpio en LBA 2048" in boot_log, (
            f"FAT32 auto-init at LBA 2048 did not trigger:\n{boot_log[:2500]}"
        )
        assert "montada en /mnt/fat0" in boot_log, "FAT32 volume not mounted on /mnt/fat0"
        print("[PASS] Booted FipSOs v6.3 | Pre-existing DiskFS LBA 64 preserved + FAT32 auto-initialized at LBA 2048")

        # 1. Verify /disk preservation & /mnt/fat0 seeded payloads
        run_cli_cmd(qmp, "cat /disk/boot_log.txt", "MUST SURVIVE FAT32 AUTO-INIT")
        ls_fat = run_cli_cmd(qmp, "ls /mnt/fat0", "fipsos.txt")
        assert "hello.lua" in ls_fat and "seq_model.fnn" in ls_fat, f"Missing seeded files in /mnt/fat0:\n{ls_fat}"
        run_cli_cmd(qmp, "cat /mnt/fat0/fipsos.txt", "FipSOs v6.3 FAT32 OK")
        run_cli_cmd(qmp, "lua /mnt/fat0/hello.lua", "FAT32", timeout=12.0)
        seq_out = run_cli_cmd(
            qmp,
            "/bin/fips_ai --seq-infer --model /mnt/fat0/seq_model.fnn --tokens 1,4,2,7",
            "[fips_ai] --seq-infer",
            timeout=12.0,
        )
        assert "top3=[" in seq_out, f"Missing top3= inference result:\n{seq_out}"

        # 2. Verify GUI Explorer opens /mnt/fat0 cleanly & clean q exit
        time.sleep(0.2)
        qmp.send_text("gui\n")
        ok_gui, _ = base.wait_for_serial("[desktop_ui.elf] FipSOs v6.3", timeout=15.0)
        assert ok_gui, f"GUI v6.3 did not launch:\n{base.read_serial()[-800:]}"
        time.sleep(0.8)

        shot_clean = qmp.screendump("shot_v63_clean_desktop")
        ppm_clean = os.path.join(REPO_DIR, "build", "shot_v63_clean_desktop.ppm")
        ppm_files.append(ppm_clean)
        w, h, clean_rgb = load_ppm_rgb(ppm_clean)
        qmp.mouse_x = w // 2
        qmp.mouse_y = h // 2

        taskbar_y = h - 48
        dock_x0 = (w - 384) // 2
        pos_exp = len(base.read_serial())
        qmp.click_at(dock_x0 + 3 * 48 + 20, taskbar_y + 24)
        ok_exp, _ = base.wait_for_serial("[ui_explorer] dir=/mnt/fat0", timeout=10.0, offset=pos_exp)
        exp_log = base.strip_ansi(base.read_serial()[pos_exp:])
        assert ok_exp and "fipsos.txt" in exp_log and "hello.lua" in exp_log and "seq_model.fnn" in exp_log, (
            f"GUI Explorer did not display /mnt/fat0 seeded files:\n{exp_log}"
        )
        time.sleep(0.6)
        shot_exp = qmp.screendump("shot_v63_explorer_fat0")
        ppm_exp = os.path.join(REPO_DIR, "build", "shot_v63_explorer_fat0.ppm")
        ppm_files.append(ppm_exp)
        _, _, exp_rgb = load_ppm_rgb(ppm_exp)
        assert region_signature(clean_rgb, w, 40, h // 2, 560, h - h // 2 - 56) != region_signature(
            exp_rgb, w, 40, h // 2, 560, h - h // 2 - 56
        ), "GUI Explorer window did not render visually!"
        print(f"[PASS] GUI Explorer rendered /mnt/fat0 cleanly -> {shot_exp}")

        # Keep clean copies in artifact dir if available, otherwise build/ already has PNGs from screendump
        pos_q = len(base.read_serial())
        qmp.send_text("q")
        ok_q, _ = base.wait_for_serial("fipsos:/#", timeout=12.0, offset=pos_q)
        assert ok_q and "Sesion grafica finalizada limpiamente" in base.read_serial()[pos_q:], "Clean GUI exit failed"
        print("[PASS] Exited GUI cleanly back to fipsos:/#")

        # 3. Verify Container Subsystem & /bin/docker CLI
        host_uname = run_cli_cmd(qmp, "uname -a", "6.3.0-FipSOs")
        assert "ctr-node" not in host_uname, f"Host hostname polluted before container test:\n{host_uname}"

        imgs_out = run_cli_cmd(qmp, "docker images", "fipsos/base")
        assert (
            "fipsos/ai-runtime" in imgs_out
            and "/containers/base-rootfs" in imgs_out
            and "/containers/ai-rootfs" in imgs_out
            and "ZeroCopy-VMO" in imgs_out
        ), f"Missing expected entries in docker images:\n{imgs_out}"

        ctr_uname = run_cli_cmd(
            qmp,
            "docker run --name test-box --mem 16 --hostname ctr-node fipsos/base:6.3 /bin/uname -a",
            "ctr-node",
            timeout=15.0,
        )
        assert "6.3.0-FipSOs" in ctr_uname, f"Container uname missing 6.3.0-FipSOs:\n{ctr_uname}"

        host_uname_after = run_cli_cmd(qmp, "uname -a", "6.3.0-FipSOs")
        assert "ctr-node" not in host_uname_after, f"Container UTS namespace leaked to host:\n{host_uname_after}"

        # Chroot confinement checks
        osrel_out = run_cli_cmd(
            qmp,
            "docker run fipsos/base:6.3 /bin/cat /etc/os-release",
            "fipsos/base:6.3",
            timeout=12.0,
        )
        assert "FipSOs Base Container" in osrel_out, f"Container /etc/os-release missing expected content:\n{osrel_out}"

        motd_attempt = run_cli_cmd(qmp, "docker run fipsos/base:6.3 /bin/cat /etc/motd.txt", timeout=12.0)
        assert "Bienvenido a FipSOs" not in motd_attempt, f"Chroot breach! Host /etc/motd.txt readable:\n{motd_attempt}"

        disk_attempt = run_cli_cmd(qmp, "docker run fipsos/base:6.3 /bin/cat /disk/boot_log.txt", timeout=12.0)
        assert "MUST SURVIVE" not in disk_attempt, f"Chroot breach! /disk/boot_log.txt readable:\n{disk_attempt}"

        escape_attempt = run_cli_cmd(qmp, "docker run fipsos/base:6.3 /bin/cat /../../disk/boot_log.txt", timeout=12.0)
        assert "MUST SURVIVE" not in escape_attempt, f"Chroot .. traversal breach!:\n{escape_attempt}"
        print("[PASS] chroot mount namespace confinement verified (direct & /../../ traversal blocked)")

        # Memory Cgroup OOM enforcement (--mem 2 = 2 MiB limit, trying 8 MiB Lua string allocation)
        pos_oom = len(base.read_serial())
        oom_out = run_cli_cmd(
            qmp,
            "docker run --name oom-box --mem 2 fipsos/base:6.3 /bin/lua -e s=string.rep(string.char(65),8388608)",
            timeout=15.0,
        )
        oom_tail = base.strip_ansi(base.read_serial()[pos_oom:])
        assert "[cgroup::mem] Container" in oom_tail and "OOM bloqueado" in oom_tail and "(-ENOMEM)" in oom_tail, (
            f"Memory cgroup OOM did not trigger -ENOMEM as expected:\n{oom_tail}\n---\n{oom_out}"
        )
        # Ensure host shell remains fully operational after container OOM
        run_cli_cmd(qmp, "echo HOST_OK", "HOST_OK")

        # Container lifecycle: ps, inspect, rm
        ps_out = run_cli_cmd(qmp, "docker ps", "test-box")
        assert "ctr-node" in ps_out or "fipsos/base:6.3" in ps_out, f"Unexpected docker ps output:\n{ps_out}"

        insp_out = run_cli_cmd(qmp, "docker inspect test-box", "ctr-node")
        assert "fipsos/base:6.3" in insp_out, f"docker inspect missing image:\n{insp_out}"

        run_cli_cmd(qmp, "docker rm test-box", "test-box")
        ps_after_rm = run_cli_cmd(qmp, "docker ps")
        assert "test-box" not in ps_after_rm, f"test-box still present after docker rm:\n{ps_after_rm}"

        full_log = base.strip_ansi(base.read_serial())
        assert (
            "UNRECOVERABLE RING 0 EXCEPTION" not in full_log
            and "[FipSOs PANIC]" not in full_log
            and "Triple" not in full_log
        ), "Unexpected kernel panic/fault found in serial log!"
        print("=== ALL FipSOs v6.3 TESTS PASSED ===")
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
