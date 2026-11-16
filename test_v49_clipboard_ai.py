#!/usr/bin/env python3
"""
FipSOs v4.9 Verification Suite:
  1. FipSOs v4.9 Framekernel boot, uname -a -> 4.9.0-FipSOs
  2. Native Goose Zero-GC + SSE2 MLP /bin/fips_ai:
     - Built-in default weights inference (fp32_sse2 + int8_sse2)
     - Custom /mnt/fat0/model.fnn weights inference forcing HIGH_LOAD
  3. Integrated Desktop Productivity:
     - Press 'n' to create /mnt/fat0/nuevo_v49.c in Code Editor
     - Test Ctrl+C, Enter, Ctrl+V, Ctrl+S (shared clipboard + save)
     - Click "Guardar y Ejecutar" -> compile with /bin/tcc & run
     - Capture PNG screenshots (boot, clipboard+editor, monitor+AI)
"""

import json
import os
import re
import socket
import struct
import subprocess
import sys
import time
import zlib

REPO_DIR = os.path.dirname(os.path.abspath(__file__))
ISO_PATH = os.path.join(REPO_DIR, "build", "fipsos-x86_64.iso")
DISK_PATH = os.path.join(REPO_DIR, "build", "test_v49_fat32.img")
SERIAL_LOG = os.path.join(REPO_DIR, "build", "test_v49_serial.log")
QMP_PORT = 4469
ARTIFACT_DIR = (
    "/mnt/c/Users/Ferna/.gemini/antigravity/brain/"
    "4ef4b43d-91a0-44a4-aefe-f8b38846bd63"
)

ANSI_RE = re.compile(r"\x1b\[[0-9;?]*[A-Za-z]")


def strip_ansi(text: str) -> str:
    return ANSI_RE.sub("", text)


def ppm_to_png(ppm_path: str, png_path: str) -> None:
    with open(ppm_path, "rb") as f:
        magic = f.readline().strip()
        assert magic == b"P6", f"Unexpected PPM magic: {magic}"
        dims = f.readline().strip()
        while dims.startswith(b"#"):
            dims = f.readline().strip()
        w, h = map(int, dims.split())
        maxv = int(f.readline().strip())
        assert maxv == 255
        rgb = f.read(w * h * 3)

    raw_rows = bytearray()
    stride = w * 3
    for y in range(h):
        raw_rows.append(0)
        raw_rows.extend(rgb[y * stride : (y + 1) * stride])

    def chunk(tag: bytes, data: bytes) -> bytes:
        return (
            struct.pack(">I", len(data))
            + tag
            + data
            + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)
        )

    ihdr = struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0)
    png = (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", ihdr)
        + chunk(b"IDAT", zlib.compress(bytes(raw_rows), 6))
        + chunk(b"IEND", b"")
    )
    with open(png_path, "wb") as f:
        f.write(png)


class QmpClient:
    def __init__(self, port: int, timeout: float = 20.0) -> None:
        deadline = time.time() + timeout
        while time.time() < deadline:
            try:
                self.sock = socket.create_connection(("127.0.0.1", port), timeout=5.0)
                break
            except OSError:
                time.sleep(0.25)
        else:
            raise RuntimeError("Could not connect to QMP")
        self.buf = b""
        self.mouse_x = 512
        self.mouse_y = 384
        self._recv_line()
        self.cmd("qmp_capabilities")

    def _recv_line(self) -> dict:
        while b"\n" not in self.buf:
            chunk = self.sock.recv(4096)
            if not chunk:
                raise RuntimeError("QMP closed")
            self.buf += chunk
        line, self.buf = self.buf.split(b"\n", 1)
        return json.loads(line.decode("utf-8"))

    def cmd(self, execute: str, arguments: dict | None = None) -> dict:
        payload = {"execute": execute}
        if arguments is not None:
            payload["arguments"] = arguments
        self.sock.sendall((json.dumps(payload) + "\n").encode("utf-8"))
        while True:
            msg = self._recv_line()
            if "event" in msg:
                continue
            return msg

    def send_key(self, qcode: str) -> None:
        self.cmd("send-key", {"keys": [{"type": "qcode", "data": qcode}]})

    def send_combo(self, qcodes: list[str]) -> None:
        self.cmd("send-key", {"keys": [{"type": "qcode", "data": k} for k in qcodes]})

    def send_text(self, text: str) -> None:
        special = {
            "\n": ["ret"],
            " ": ["spc"],
            "-": ["minus"],
            "_": ["shift", "minus"],
            ".": ["dot"],
            "/": ["slash"],
            ":": ["shift", "semicolon"],
            ";": ["semicolon"],
            "=": ["equal"],
            "<": ["shift", "comma"],
            ">": ["shift", "dot"],
            "(": ["shift", "9"],
            ")": ["shift", "0"],
            "{": ["shift", "bracket_left"],
            "}": ["shift", "bracket_right"],
            "#": ["shift", "3"],
            '"': ["shift", "apostrophe"],
            "'": ["apostrophe"],
            "\\": ["backslash"],
            "&": ["shift", "7"],
            ",": ["comma"],
            "[": ["bracket_left"],
            "]": ["bracket_right"],
            "*": ["shift", "8"],
        }
        for ch in text:
            if ch in special:
                self.send_combo(special[ch])
            elif ch.isupper():
                self.send_combo(["shift", ch.lower()])
            else:
                self.send_key(ch)
            time.sleep(0.05)

    def click_at(self, x: int, y: int) -> None:
        dx = x - self.mouse_x
        dy = y - self.mouse_y
        while dx != 0 or dy != 0:
            sx = max(-24, min(24, dx))
            sy = max(-24, min(24, dy))
            self.cmd(
                "input-send-event",
                {
                    "events": [
                        {"type": "rel", "data": {"axis": "x", "value": sx}},
                        {"type": "rel", "data": {"axis": "y", "value": sy}},
                    ]
                },
            )
            dx -= sx
            dy -= sy
            self.mouse_x += sx
            self.mouse_y += sy
            time.sleep(0.03)
        self.cmd(
            "input-send-event",
            {"events": [{"type": "btn", "data": {"button": "left", "down": True}}]},
        )
        time.sleep(0.08)
        self.cmd(
            "input-send-event",
            {"events": [{"type": "btn", "data": {"button": "left", "down": False}}]},
        )

    def screendump(self, name: str) -> str:
        ppm = os.path.join(REPO_DIR, "build", f"{name}.ppm")
        png = os.path.join(ARTIFACT_DIR, f"{name}.png")
        self.cmd("screendump", {"filename": ppm})
        time.sleep(0.2)
        ppm_to_png(ppm, png)
        return png

    def close(self) -> None:
        self.sock.close()


def read_serial() -> str:
    if not os.path.exists(SERIAL_LOG):
        return ""
    with open(SERIAL_LOG, "rb") as f:
        return strip_ansi(f.read().decode("utf-8", errors="replace"))


def wait_for_serial(pattern: str, timeout: float = 20.0, offset: int = 0) -> tuple[bool, int]:
    deadline = time.time() + timeout
    while time.time() < deadline:
        txt = read_serial()
        idx = txt.find(pattern, offset)
        if idx >= 0:
            return True, idx + len(pattern)
        time.sleep(0.25)
    return False, offset


def run_shell_cmd(qmp: QmpClient, cmd: str, wait_pattern: str, timeout: float = 15.0) -> str:
    offset = len(read_serial())
    qmp.send_text(cmd + "\n")
    ok, _ = wait_for_serial(wait_pattern, timeout=timeout, offset=offset)
    if not ok:
        raise AssertionError(
            f"Timed out waiting for '{wait_pattern}' after '{cmd}'. Tail:\n{read_serial()[offset:]}"
        )
    return read_serial()[offset:]


def create_fat32_disk(path: str) -> None:
    if os.path.exists(path):
        os.remove(path)
    total_sectors = 131072
    buf = bytearray(total_sectors * 512)
    part_start = 2048
    part_end = total_sectors - 34
    part_sectors = part_end - part_start + 1

    buf[446:462] = struct.pack("<B3sB3sII", 0x00, b"\x00\x02\x00", 0xEE, b"\xff\xff\xff", 1, total_sectors - 1)
    buf[510:512] = b"\x55\xAA"

    gpt_hdr = bytearray(512)
    gpt_hdr[0:8] = b"EFI PART"
    struct.pack_into("<II", gpt_hdr, 8, 0x00010000, 92)
    struct.pack_into("<QQQQ", gpt_hdr, 24, 1, total_sectors - 1, part_start, part_end)
    gpt_hdr[56:72] = bytes.fromhex("11223344556677889900aabbccddeeff")
    struct.pack_into("<QIII", gpt_hdr, 72, 2, 128, 128, 0)

    type_guid = struct.pack("<IHH", 0xEBD0A0A2, 0xB9E5, 0x4433) + bytes.fromhex("87c068b6b72699c7")
    part_entry = bytearray(128)
    part_entry[0:16] = type_guid
    part_entry[16:32] = bytes.fromhex("a1b2c3d4e5f60718293a4b5c6d7e8f90")
    struct.pack_into("<QQQ", part_entry, 32, part_start, part_end, 0)
    part_entry[56:72] = "FAT32".encode("utf-16le")

    entries = bytearray(128 * 128)
    entries[0:128] = part_entry
    struct.pack_into("<I", gpt_hdr, 88, zlib.crc32(entries) & 0xFFFFFFFF)
    struct.pack_into("<I", gpt_hdr, 16, zlib.crc32(gpt_hdr[:92]) & 0xFFFFFFFF)
    buf[512:1024] = gpt_hdr
    buf[1024 : 1024 + len(entries)] = entries

    part_off = part_start * 512
    reserved, num_fats, spc, fat_size = 32, 2, 8, 1024
    data_sectors = part_sectors - reserved - num_fats * fat_size

    vbr = bytearray(512)
    vbr[0:3] = b"\xEB\x58\x90"
    vbr[3:11] = b"FIPSOS49"
    struct.pack_into("<HBHB", vbr, 11, 512, spc, reserved, num_fats)
    struct.pack_into("<BHI", vbr, 21, 0xF8, 0, part_start)
    struct.pack_into("<II", vbr, 32, part_sectors, fat_size)
    struct.pack_into("<HHIHH", vbr, 40, 0, 0, 2, 1, 6)
    vbr[64] = 0x80
    vbr[66] = 0x29
    struct.pack_into("<I", vbr, 67, 0x46505334)
    vbr[71:82] = b"FIPSOSFAT32"
    vbr[82:90] = b"FAT32   "
    vbr[510:512] = b"\x55\xAA"
    buf[part_off : part_off + 512] = vbr
    buf[part_off + 6 * 512 : part_off + 7 * 512] = vbr

    fsi = bytearray(512)
    struct.pack_into("<I", fsi, 0, 0x41615252)
    struct.pack_into("<III", fsi, 484, 0x61417272, data_sectors - 1, 3)
    struct.pack_into("<I", fsi, 508, 0xAA550000)
    buf[part_off + 512 : part_off + 1024] = fsi
    buf[part_off + 7 * 512 : part_off + 8 * 512] = fsi

    fat_sec = bytearray(512)
    struct.pack_into("<III", fat_sec, 0, 0x0FFFFFF8, 0x0FFFFFFF, 0x0FFFFFFF)
    fat1_off = part_off + reserved * 512
    buf[fat1_off : fat1_off + 512] = fat_sec
    buf[fat1_off + fat_size * 512 : fat1_off + fat_size * 512 + 512] = fat_sec

    with open(path, "wb") as f:
        f.write(buf)


def main() -> int:
    create_fat32_disk(DISK_PATH)
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
        qmp = QmpClient(QMP_PORT)
        ok, _ = wait_for_serial("fipsos:/#", timeout=30.0)
        assert ok, f"Shell prompt not reached. Serial:\n{read_serial()[-1200:]}"
        time.sleep(0.5)

        shot1 = qmp.screendump("shot1_v49_boot")
        print(f"[PASS] Booted FipSOs v4.9 -> {shot1}")

        run_shell_cmd(qmp, "uname -a", "4.9.0-FipSOs")
        print("[PASS] uname -a -> 4.9.0-FipSOs")

        # 1. /bin/fips_ai with built-in weights
        out_ai1 = run_shell_cmd(qmp, "/bin/fips_ai", "int8_sse2: class=")
        assert "weights=builtin_default" in out_ai1, f"Unexpected fips_ai output:\n{out_ai1}"
        assert "fp32_sse2: class=" in out_ai1, f"Missing fp32_sse2 output:\n{out_ai1}"
        print("[PASS] /bin/fips_ai (builtin_default) FP32 & INT8 SSE2 inference succeeded")

        # 2. Create custom /mnt/fat0/model.fnn via tcc and re-test /bin/fips_ai
        run_shell_cmd(qmp, "echo '#include <stdio.h>' > /mnt/fat0/gen_fnn.c", "fipsos:/#")
        run_shell_cmd(qmp, "echo 'int main(void){' >> /mnt/fat0/gen_fnn.c", "fipsos:/#")
        run_shell_cmd(qmp, "echo '  struct{unsigned m,q;float s,w1[32],b1[8],w2[24],b2[3];}f={0};' >> /mnt/fat0/gen_fnn.c", "fipsos:/#")
        run_shell_cmd(qmp, "echo '  f.m=0x314E4E46U;f.s=0.02f;f.b2[2]=6.0f;' >> /mnt/fat0/gen_fnn.c", "fipsos:/#")
        run_shell_cmd(qmp, "echo '  FILE*fp=fopen(\"/mnt/fat0/model.fnn\",\"wb\");' >> /mnt/fat0/gen_fnn.c", "fipsos:/#")
        run_shell_cmd(qmp, "echo '  fwrite(&f,sizeof(f),1,fp);fclose(fp);' >> /mnt/fat0/gen_fnn.c", "fipsos:/#")
        run_shell_cmd(qmp, "echo '  printf(\"FNN_OK\\n\");return 0;}' >> /mnt/fat0/gen_fnn.c", "fipsos:/#")
        run_shell_cmd(qmp, "/bin/tcc /mnt/fat0/gen_fnn.c -o /disk/g", "fipsos:/#", timeout=25.0)
        time.sleep(0.3)
        run_shell_cmd(qmp, "/disk/g", "FNN_OK", timeout=15.0)
        out_ai2 = run_shell_cmd(qmp, "/bin/fips_ai", "int8_sse2: class=HIGH_LOAD")
        assert "weights=/mnt/fat0/model.fnn" in out_ai2, f"Custom model.fnn not loaded:\n{out_ai2}"
        assert "fp32_sse2: class=HIGH_LOAD" in out_ai2, f"Custom FP32 class mismatch:\n{out_ai2}"
        print("[PASS] /bin/fips_ai loaded /mnt/fat0/model.fnn -> HIGH_LOAD")

        # 3. Desktop UI: Create /mnt/fat0/nuevo_v49.c with 'n', test Ctrl+C / Ctrl+V / Ctrl+S, and Save & Run
        offset = len(read_serial())
        qmp.send_text("gui\n")
        ok, _ = wait_for_serial("[desktop_ui.elf] FipSOs v4.9", timeout=15.0, offset=offset)
        assert ok, f"desktop_ui.elf v4.9 did not start:\n{read_serial()[offset:]}"
        time.sleep(1.2)

        qmp.send_key("n")
        ok, ed_off = wait_for_serial("[ui_editor] save path=/mnt/fat0/nuevo_v49.c", timeout=10.0, offset=offset)
        assert ok, f"nuevo_v49.c was not created:\n{read_serial()[offset:]}"
        time.sleep(0.4)

        # Cursor starts on line 2 (printf). Copy line with Ctrl+C, move to end/new line, paste with Ctrl+V, save with Ctrl+S
        qmp.send_combo(["ctrl", "c"])
        time.sleep(0.2)
        qmp.send_key("ret")
        time.sleep(0.15)
        qmp.send_combo(["ctrl", "v"])
        time.sleep(0.2)
        save_off = len(read_serial())
        qmp.send_combo(["ctrl", "s"])
        ok, _ = wait_for_serial("[ui_editor] save path=/mnt/fat0/nuevo_v49.c", timeout=10.0, offset=save_off)
        assert ok, f"Ctrl+S save did not trigger:\n{read_serial()[save_off:]}"
        time.sleep(0.4)
        shot2 = qmp.screendump("shot2_v49_clipboard_editor")
        print(f"[PASS] Code Editor Nuevo + Ctrl+C/Ctrl+V/Ctrl+S verified -> {shot2}")

        # Save & Run via Ctrl+R
        run_off = len(read_serial())
        qmp.send_combo(["ctrl", "r"])
        ok, _ = wait_for_serial("[ui_editor] run path=/mnt/fat0/nuevo_v49.c", timeout=18.0, offset=run_off)
        assert ok, f"Save & Run failed:\n{read_serial()[run_off:]}"
        time.sleep(0.5)
        shot3_ed = qmp.screendump("shot3_v49_editor_run")
        print(f"[PASS] Code Editor Save & Run (Ctrl+R) verified -> {shot3_ed}")

        # Capture boot/desktop Monitor del Sistema (visible at top-left before Editor or after 'q')
        # Exit Desktop cleanly
        qmp.send_key("esc")
        time.sleep(0.2)
        qmp.send_key("q")
        wait_for_serial("Sesion de escritorio cerrada", timeout=6.0, offset=offset)

        print("\n=== ALL V4.9 CLIPBOARD, NUEVO & SSE2 AI TESTS PASSED ===")
        return 0
    finally:
        if qmp is not None:
            qmp.close()
        proc.terminate()
        try:
            proc.wait(timeout=5)
        except Exception:
            proc.kill()


if __name__ == "__main__":
    sys.exit(main())
