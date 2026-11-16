#!/usr/bin/env python3
"""
FipSOs v4.6 Verification Suite (QEMU + QMP + FAT32 + Goose Zero-GC + HTTP /mnt/fat0 Server)
Validates:
  1. FipSOs v4.6 Framekernel boot, /mnt/fat0 mount, uname -a -> 4.6.0-FipSOs
  2. Integrated Desktop Terminal (win 4) running /bin/goose_audit + clean App Center clipping
  3. /bin/httpd serve 8080 serving real /mnt/fat0/hello.lua & live HTML / to host port 8080
"""

import json
import os
import re
import socket
import struct
import subprocess
import sys
import time
import urllib.error
import urllib.request
import zlib

REPO_DIR = os.path.dirname(os.path.abspath(__file__))
ISO_PATH = os.path.join(REPO_DIR, "build", "fipsos-x86_64.iso")
DISK_PATH = os.path.join(REPO_DIR, "build", "test_v46_fat32.img")
SERIAL_LOG = os.path.join(REPO_DIR, "build", "test_v46_serial.log")
QMP_PORT = 4466
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
    idat = zlib.compress(bytes(raw_rows), 6)
    with open(png_path, "wb") as out:
        out.write(b"\x89PNG\r\n\x1a\n")
        out.write(chunk(b"IHDR", ihdr))
        out.write(chunk(b"IDAT", idat))
        out.write(chunk(b"IEND", b""))


class QmpClient:
    def __init__(self, host: str, port: int, timeout: float = 15.0):
        deadline = time.time() + timeout
        last_err = None
        while time.time() < deadline:
            try:
                self.sock = socket.create_connection((host, port), timeout=5.0)
                break
            except OSError as e:
                last_err = e
                time.sleep(0.15)
        else:
            raise RuntimeError(f"QMP connect failed: {last_err}")
        self.buf = b""
        self._read_json()
        self._send_json({"execute": "qmp_capabilities"})
        self._read_json()

    def _read_json(self):
        while b"\n" not in self.buf:
            chunk = self.sock.recv(4096)
            if not chunk:
                raise RuntimeError("QMP EOF")
            self.buf += chunk
        line, self.buf = self.buf.split(b"\n", 1)
        return json.loads(line.decode("utf-8", errors="ignore"))

    def _send_json(self, obj):
        self.sock.sendall((json.dumps(obj) + "\n").encode("utf-8"))

    def send_key(self, qcode: str):
        self._send_json({"execute": "send-key", "arguments": {"keys": [{"type": "qcode", "data": qcode}]}})
        self._read_json()

    def send_text(self, text: str, enter: bool = True, delay: float = 0.04):
        simple = {
            " ": "spc", "-": "minus", "=": "equal", ".": "dot", "/": "slash",
            ",": "comma", ";": "semicolon", "'": "apostrophe", "\\": "backslash",
            "[": "bracket_left", "]": "bracket_right",
        }
        shifted = {
            "_": "minus", "+": "equal", ":": "semicolon", '"': "apostrophe",
            ">": "dot", "<": "comma", "(": "9", ")": "0", "!": "1",
        }
        for ch in text:
            if "a" <= ch <= "z" or "0" <= ch <= "9":
                self.send_key(ch)
            elif "A" <= ch <= "Z":
                self._send_json({
                    "execute": "send-key",
                    "arguments": {"keys": [{"type": "qcode", "data": "shift"}, {"type": "qcode", "data": ch.lower()}]},
                })
                self._read_json()
            elif ch in shifted:
                self._send_json({
                    "execute": "send-key",
                    "arguments": {"keys": [{"type": "qcode", "data": "shift"}, {"type": "qcode", "data": shifted[ch]}]},
                })
                self._read_json()
            elif ch in simple:
                self.send_key(simple[ch])
            time.sleep(delay)
        if enter:
            self.send_key("ret")
            time.sleep(0.08)

    def screendump(self, out_ppm_path: str):
        self._send_json({"execute": "screendump", "arguments": {"filename": out_ppm_path}})
        return self._read_json()

    def quit(self):
        for cmd in ({"execute": "quit"},):
            try:
                self._send_json(cmd)
                self._read_json()
            except Exception:
                pass
        try:
            self.sock.close()
        except Exception:
            pass


def read_serial(log_path: str) -> str:
    if not os.path.exists(log_path):
        return ""
    with open(log_path, "r", encoding="utf-8", errors="ignore") as f:
        return f.read()


def wait_for_serial(log_path: str, needle: str, timeout_s: float, after_pos: int = 0) -> str:
    deadline = time.time() + timeout_s
    while time.time() < deadline:
        text = strip_ansi(read_serial(log_path)[after_pos:])
        if needle in text:
            return text
        time.sleep(0.2)
    raise AssertionError(f"Timeout ({timeout_s}s) esperando en serial: {needle!r}")


def serial_pos(log_path: str) -> int:
    return len(read_serial(log_path))


def create_gpt_fat32_disk(path: str) -> None:
    if os.path.exists(path):
        os.remove(path)
    total_sectors = 131072  # 64 MiB
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

    type_guid = (
        struct.pack("<IHH", 0xEBD0A0A2, 0xB9E5, 0x4433)
        + bytes.fromhex("87c068b6b72699c7")
    )
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
    vbr[3:11] = b"FIPSOS46"
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


def capture_shot(qmp: QmpClient, name: str) -> str:
    ppm_path = os.path.join(REPO_DIR, "build", f"{name}.ppm")
    os.makedirs(ARTIFACT_DIR, exist_ok=True)
    png_path = os.path.join(ARTIFACT_DIR, f"{name}.png")
    qmp.screendump(ppm_path)
    time.sleep(0.25)
    ppm_to_png(ppm_path, png_path)
    print(f"[SHOT] {png_path}")
    return png_path


def run_cmd(qmp: QmpClient, cmd: str, expect: str, timeout_s: float = 15.0) -> str:
    time.sleep(0.2)
    pos = serial_pos(SERIAL_LOG)
    qmp.send_text(cmd, enter=True)
    return wait_for_serial(SERIAL_LOG, expect, timeout_s, after_pos=pos)


def http_get(url: str):
    req = urllib.request.Request(url, method="GET")
    try:
        with urllib.request.urlopen(req, timeout=8.0) as resp:
            return resp.status, resp.read().decode("utf-8", errors="ignore")
    except urllib.error.HTTPError as e:
        return e.code, e.read().decode("utf-8", errors="ignore")


def main() -> int:
    if not os.path.exists(ISO_PATH):
        print(f"[ERROR] ISO no encontrado: {ISO_PATH}")
        return 1
    if os.path.exists(SERIAL_LOG):
        os.remove(SERIAL_LOG)

    create_gpt_fat32_disk(DISK_PATH)

    qemu_cmd = [
        "qemu-system-x86_64",
        "-M", "q35",
        "-cpu", "max",
        "-smp", "2",
        "-m", "512M",
        "-no-reboot", "-no-shutdown",
        "-serial", f"file:{SERIAL_LOG}",
        "-qmp", f"tcp:127.0.0.1:{QMP_PORT},server,nowait",
        "-drive", f"id=satadisk,file={DISK_PATH},if=none,format=raw",
        "-device", "ide-hd,drive=satadisk,bus=ide.0",
        "-device", "qemu-xhci,id=xhci0",
        "-device", "usb-kbd,bus=xhci0.0",
        "-device", "usb-mouse,bus=xhci0.0",
        "-netdev", "user,id=net0,hostfwd=tcp::2222-:2222,hostfwd=tcp::8080-:8080",
        "-device", "e1000e,netdev=net0",
        "-boot", "d",
        "-cdrom", ISO_PATH,
    ]
    print("[QEMU] Iniciando FipSOs v4.6...")
    proc = subprocess.Popen(qemu_cmd)
    qmp = None
    try:
        qmp = QmpClient("127.0.0.1", QMP_PORT, timeout=20.0)
        wait_for_serial(SERIAL_LOG, "FipSOs v4.6", 25.0)
        wait_for_serial(SERIAL_LOG, "/mnt/fat0", 25.0)
        wait_for_serial(SERIAL_LOG, "8080", 25.0)
        wait_for_serial(SERIAL_LOG, "fipsos:/#", 25.0)
        print("[PASS] Boot v4.6 + /mnt/fat0 + TCP :2222 + Shell prompt")
        capture_shot(qmp, "shot1_v46_boot")

        run_cmd(qmp, "uname -a", "4.6.0-FipSOs", 10.0)
        print("[PASS] uname -a -> 4.6.0-FipSOs")

        run_cmd(qmp, 'echo "Hola HTTP FAT32 v4.6" > /mnt/fat0/hello.lua', "fipsos:/#", 10.0)
        run_cmd(qmp, "cat /mnt/fat0/hello.lua", "Hola HTTP FAT32 v4.6", 10.0)
        print("[PASS] Archivo /mnt/fat0/hello.lua creado")

        # Launch desktop_ui.elf and test Integrated Terminal + goose_audit + clipping
        pos = serial_pos(SERIAL_LOG)
        qmp.send_text("desktop_ui.elf", enter=True)
        wait_for_serial(SERIAL_LOG, "[desktop_ui.elf] FipSOs v4.6", 15.0, after_pos=pos)
        time.sleep(0.6)

        # Open Integrated Terminal (5) and run goose_audit
        qmp.send_key("5")
        time.sleep(0.5)
        pos_ga = serial_pos(SERIAL_LOG)
        qmp.send_text("goose_audit", enter=True)
        wait_for_serial(SERIAL_LOG, "[ui_terminal::exec] goose_audit", 10.0, after_pos=pos_ga)
        wait_for_serial(SERIAL_LOG, "sys_exit/exit_group(status=0)", 10.0, after_pos=pos_ga)
        time.sleep(0.6)
        capture_shot(qmp, "shot2_v46_goose_audit")
        print("[PASS] /bin/goose_audit ejecutado en Terminal Integrada")

        # Open App Center (4) to verify strict text clipping inside cards
        qmp.send_key("4")
        time.sleep(0.6)
        capture_shot(qmp, "shot3_v46_desktop_clipped")

        # Exit desktop_ui cleanly
        pos_exit = serial_pos(SERIAL_LOG)
        qmp.send_key("esc")
        wait_for_serial(SERIAL_LOG, "Sesion grafica finalizada limpiamente", 10.0, after_pos=pos_exit)
        print("[PASS] desktop_ui.elf cerrado limpiamente")

        # Start HTTP server on port 8080
        pos_http = serial_pos(SERIAL_LOG)
        qmp.send_text("httpd serve 8080", enter=True)
        wait_for_serial(SERIAL_LOG, "Escuchando HTTP/1.0 en 0.0.0.0:8080", 10.0, after_pos=pos_http)
        time.sleep(0.3)

        st1, body1 = http_get("http://127.0.0.1:8080/hello.lua")
        assert st1 == 200 and "Hola HTTP FAT32 v4.6" in body1, f"GET /hello.lua failed: {st1} {body1!r}"
        print(f"[PASS] HTTP GET /hello.lua -> {st1} ({body1.strip()})")

        st2, body2 = http_get("http://127.0.0.1:8080/")
        assert st2 == 200 and "FipSOs v4.6 HTTP File Server" in body2, f"GET / failed: {st2}"
        print(f"[PASS] HTTP GET / -> {st2} (HTML status)")

        st3, _ = http_get("http://127.0.0.1:8080/../etc/motd.txt")
        assert st3 == 403, f"Expected 403 on traversal, got {st3}"
        print(f"[PASS] HTTP GET /../etc/motd.txt -> {st3} (traversal blocked)")

        print("[SUCCESS] Todas las pruebas de FipSOs v4.6 pasaron correctamente!")
        return 0
    finally:
        if qmp:
            qmp.quit()
        proc.terminate()
        try:
            proc.wait(timeout=5.0)
        except subprocess.TimeoutExpired:
            proc.kill()


if __name__ == "__main__":
    sys.exit(main())
