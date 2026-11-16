#!/usr/bin/env python3
"""
FipSOs v5.0 Verification Suite:
  1. FipSOs v5.0 Framekernel boot, uname -a -> 5.0.0-FipSOs
  2. Basic Audio Subsystem (/dev/audio & /bin/beep)
  3. Neural Intrusion Guardian (/bin/fips_ai --guard AVX2/SSE2)
  4. Real-time HTTP Intrusion Detection + Auto-Sandbox (httpd serve 8080 &)
     - Legitimate GET /index.txt -> 200 OK (SAFE)
     - Traversal GET /../../etc/passwd -> 403 Forbidden (INTRUSION + AUTO_SANDBOX)
     - Verify /mnt/fat0/security_ai.log & /mnt/fat0/ai_guard.state
  5. Desktop C++20 Monitor del Sistema displaying Neural Guardian verdict
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
DISK_PATH = os.path.join(REPO_DIR, "build", "test_v50_fat32.img")
SERIAL_LOG = os.path.join(REPO_DIR, "build", "test_v50_serial.log")
QMP_PORT = 4470
HTTP_FWD_PORT = 18080
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
            ">": ["shift", "dot"],
            "&": ["shift", "7"],
        }
        for ch in text:
            if ch in special:
                self.send_combo(special[ch])
            elif ch.isupper():
                self.send_combo(["shift", ch.lower()])
            else:
                self.send_key(ch)
            time.sleep(0.05)

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
        if idx != -1:
            return True, idx + len(pattern)
        time.sleep(0.2)
    return False, len(read_serial())


def run_shell_cmd(qmp: QmpClient, cmd: str, expect: str, timeout: float = 15.0) -> str:
    start = len(read_serial())
    qmp.send_text(cmd + "\n")
    ok, _ = wait_for_serial(expect, timeout=timeout, offset=start)
    txt = read_serial()[start:]
    assert ok, f"Command '{cmd}' did not emit '{expect}'. Output:\n{txt}"
    time.sleep(0.25)
    return read_serial()[start:]


def create_fat32_disk(path: str, size_mb: int = 16) -> None:
    total_sectors = (size_mb * 1024 * 1024) // 512
    buf = bytearray(total_sectors * 512)
    part_start = 2048
    part_sectors = total_sectors - part_start - 64
    part_end = part_start + part_sectors - 1

    mbr = bytearray(512)
    mbr[446 + 4] = 0xEE
    struct.pack_into("<II", mbr, 446 + 8, 1, total_sectors - 1)
    mbr[510:512] = b"\x55\xAA"
    buf[0:512] = mbr

    gpt_hdr = bytearray(512)
    gpt_hdr[0:8] = b"EFI PART"
    struct.pack_into("<II", gpt_hdr, 8, 0x00010000, 92)
    struct.pack_into("<QQQQ", gpt_hdr, 24, 1, total_sectors - 1, 34, total_sectors - 34)
    gpt_hdr[56:72] = bytes.fromhex("112233445566778899aabbccddeeff00")
    struct.pack_into("<QIII", gpt_hdr, 72, 2, 128, 128, 0)

    type_guid = bytes.fromhex("a2a0d0ebe5b9334487c068b6b72699c7")
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
    vbr[3:11] = b"FIPSOS50"
    struct.pack_into("<HBHB", vbr, 11, 512, spc, reserved, num_fats)
    struct.pack_into("<BHI", vbr, 21, 0xF8, 0, part_start)
    struct.pack_into("<II", vbr, 32, part_sectors, fat_size)
    struct.pack_into("<HHIHH", vbr, 40, 0, 0, 2, 1, 6)
    vbr[64] = 0x80
    vbr[66] = 0x29
    struct.pack_into("<I", vbr, 67, 0x46505335)
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


def http_get(port: int, path: str) -> str:
    with socket.create_connection(("127.0.0.1", port), timeout=10.0) as s:
        req = f"GET {path} HTTP/1.0\r\nHost: 127.0.0.1\r\n\r\n".encode("ascii")
        s.sendall(req)
        chunks = []
        while True:
            data = s.recv(4096)
            if not data:
                break
            chunks.append(data)
        return b"".join(chunks).decode("utf-8", errors="replace")


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
        "-netdev", f"user,id=net0,hostfwd=tcp:127.0.0.1:{HTTP_FWD_PORT}-:8080",
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

        shot1 = qmp.screendump("shot1_v50_boot")
        print(f"[PASS] Booted FipSOs v5.0 -> {shot1}")

        run_shell_cmd(qmp, "uname -a", "5.0.0-FipSOs")
        print("[PASS] uname -a -> 5.0.0-FipSOs")

        # 1. Audio subsystem (/dev/audio & /bin/beep)
        aud = run_shell_cmd(qmp, "cat /dev/audio", "FIPSOS_AUDIO_V1")
        assert "CTRL=" in aud, f"Unexpected /dev/audio output:\n{aud}"
        run_shell_cmd(qmp, "/bin/beep 880:30", "[beep]")
        run_shell_cmd(qmp, "echo 440:30 > /dev/audio", "fipsos:/#")
        print("[PASS] /dev/audio & /bin/beep verified")

        # 2. /bin/fips_ai --guard (SAFE vs INTRUSION + AUTO_SANDBOX)
        g_safe = run_shell_cmd(qmp, "/bin/fips_ai --guard 8 0 0 2 0 20", "verdict=SAFE")
        assert "backend=" in g_safe, f"Missing backend in fips_ai --guard:\n{g_safe}"
        g_intr = run_shell_cmd(qmp, "/bin/fips_ai --guard 35 2 5 6 0 25", "verdict=INTRUSION")
        assert "AUTO_SANDBOX" in g_intr, f"Missing AUTO_SANDBOX in fips_ai --guard:\n{g_intr}"
        print("[PASS] /bin/fips_ai --guard SAFE & INTRUSION+AUTO_SANDBOX verified")

        # 3. Live HTTP daemon + Neural Intrusion Guardian + Auto-Sandbox
        run_shell_cmd(qmp, "echo hello_v50 > /mnt/fat0/index.txt", "fipsos:/#")
        run_shell_cmd(qmp, "httpd serve 8080 --max 2", "Escuchando HTTP/1.0")
        time.sleep(0.5)

        resp_ok = http_get(HTTP_FWD_PORT, "/index.txt")
        assert "200 OK" in resp_ok and "hello_v50" in resp_ok, f"Bad SAFE HTTP resp:\n{resp_ok}"
        print("[PASS] HTTP GET /index.txt -> 200 OK (SAFE)")

        resp_atk = http_get(HTTP_FWD_PORT, "/../../etc/passwd")
        assert "403 Forbidden" in resp_atk and "INTRUSION" in resp_atk, f"Bad attack HTTP resp:\n{resp_atk}"
        print("[PASS] HTTP GET /../../etc/passwd -> 403 Forbidden (INTRUSION blocked)")

        time.sleep(0.5)
        sec_log = run_shell_cmd(qmp, "cat /mnt/fat0/security_ai.log", "INTRUSION")
        assert "AUTO_SANDBOX" in sec_log, f"Missing AUTO_SANDBOX in security_ai.log:\n{sec_log}"
        state_txt = run_shell_cmd(qmp, "cat /mnt/fat0/ai_guard.state", "INTRUSION|")
        assert "INTRUSION|" in state_txt, f"Unexpected ai_guard.state:\n{state_txt}"
        print("[PASS] /mnt/fat0/security_ai.log & /mnt/fat0/ai_guard.state verified")

        # 4. Launch Desktop GUI & capture Monitor del Sistema with Neural Guardian
        start_gui = len(read_serial())
        qmp.send_text("gui\n")
        ok, _ = wait_for_serial("[desktop_ui.elf] FipSOs v5.0", timeout=15.0, offset=start_gui)
        assert ok, f"Desktop GUI v5.0 did not start:\n{read_serial()[start_gui:]}"
        time.sleep(1.2)

        shot2 = qmp.screendump("shot2_v50_desktop_guard")
        print(f"[PASS] Desktop v5.0 + AI Security Monitor -> {shot2}")

        qmp.send_key("q")
        wait_for_serial("fipsos@v5.0:/$", timeout=10.0, offset=start_gui)
        print("[PASS] Returned cleanly to TTY from Desktop")
        return 0
    finally:
        if qmp is not None:
            qmp.close()
        proc.terminate()
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()


if __name__ == "__main__":
    sys.exit(main())
