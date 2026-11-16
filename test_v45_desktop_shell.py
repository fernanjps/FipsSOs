#!/usr/bin/env python3
"""
FipSOs v4.5 Verification Suite (QEMU + QMP + FAT32 + Integrated Desktop Terminal + fipsos.cfg)
Validates:
  1. FipSOs v4.5 Framekernel boot, /mnt/fat0 mount, TCP :2222, uname -a -> 4.5.0-FipSOs
  2. Integrated Desktop Terminal (win 4) inside desktop_ui.elf: ls, lua, theme slate, savecfg
  3. Persistence of /mnt/fat0/fipsos.cfg across desktop_ui restarts (cfg_loaded theme=slate)
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
DISK_PATH = os.path.join(REPO_DIR, "build", "test_v45_fat32.img")
SERIAL_LOG = os.path.join(REPO_DIR, "build", "test_v45_serial.log")
QMP_PORT = 4465
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
        crc = zlib.crc32(tag + data) & 0xFFFFFFFF
        return struct.pack(">I", len(data)) + tag + data + struct.pack(">I", crc)

    png = bytearray(b"\x89PNG\r\n\x1a\n")
    png.extend(chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0)))
    png.extend(chunk(b"IDAT", zlib.compress(bytes(raw_rows), 6)))
    png.extend(chunk(b"IEND", b""))
    with open(png_path, "wb") as out:
        out.write(png)


class QmpClient:
    def __init__(self, host: str, port: int, timeout: float = 15.0):
        self.sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        self.sock.settimeout(timeout)
        deadline = time.time() + timeout
        while True:
            try:
                self.sock.connect((host, port))
                break
            except OSError:
                if time.time() > deadline:
                    raise
                time.sleep(0.15)
        self.buf = b""
        self._read_json()
        self._send_json({"execute": "qmp_capabilities"})
        self._read_json()

    def _send_json(self, obj: dict) -> None:
        self.sock.sendall((json.dumps(obj) + "\n").encode("utf-8"))

    def _read_json(self) -> dict:
        while b"\n" not in self.buf:
            chunk = self.sock.recv(4096)
            if not chunk:
                raise RuntimeError("QMP EOF")
            self.buf += chunk
        line, self.buf = self.buf.split(b"\n", 1)
        return json.loads(line.decode("utf-8"))

    def send_key(self, qcode: str) -> None:
        self._send_json(
            {
                "execute": "send-key",
                "arguments": {"keys": [{"type": "qcode", "data": qcode}]},
            }
        )
        self._read_json()

    def send_text(self, text: str, enter: bool = True, delay: float = 0.04) -> None:
        shift_map = {
            "!": "1", "@": "2", "#": "3", "$": "4", "%": "5", "^": "6",
            "&": "7", "*": "8", "(": "9", ")": "0", "_": "minus", "+": "equal",
            "{": "bracket_left", "}": "bracket_right", "|": "backslash",
            ":": "semicolon", '"': "apostrophe", "<": "comma", ">": "dot",
            "?": "slash", "~": "grave_accent",
        }
        simple_map = {
            " ": "spc", "-": "minus", "=": "equal", "[": "bracket_left",
            "]": "bracket_right", "\\": "backslash", ";": "semicolon",
            "'": "apostrophe", ",": "comma", ".": "dot", "/": "slash",
            "`": "grave_accent", "\n": "ret",
        }
        for ch in text:
            if "a" <= ch <= "z" or "0" <= ch <= "9":
                self.send_key(ch)
            elif "A" <= ch <= "Z":
                self._send_json(
                    {
                        "execute": "send-key",
                        "arguments": {
                            "keys": [
                                {"type": "qcode", "data": "shift"},
                                {"type": "qcode", "data": ch.lower()},
                            ]
                        },
                    }
                )
                self._read_json()
            elif ch in shift_map:
                self._send_json(
                    {
                        "execute": "send-key",
                        "arguments": {
                            "keys": [
                                {"type": "qcode", "data": "shift"},
                                {"type": "qcode", "data": shift_map[ch]},
                            ]
                        },
                    }
                )
                self._read_json()
            elif ch in simple_map:
                self.send_key(simple_map[ch])
            time.sleep(delay)
        if enter:
            self.send_key("ret")
            time.sleep(0.08)

    def screendump(self, out_ppm_path: str):
        self._send_json({"execute": "screendump", "arguments": {"filename": out_ppm_path}})
        return self._read_json()

    def quit(self):
        try:
            self._send_json({"execute": "quit"})
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
        raw = read_serial(log_path)
        text = strip_ansi(raw[after_pos:])
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

    buf[446:462] = struct.pack(
        "<B3sB3sII", 0x00, b"\x00\x02\x00", 0xEE, b"\xff\xff\xff", 1, total_sectors - 1
    )
    buf[510:512] = b"\x55\xAA"

    entries = bytearray(128 * 128)
    type_guid = bytes.fromhex("a2a0d0ebe5b9334487c068b6b72699c7")
    uniq_guid = bytes.fromhex("112233445566778899aabbccddeeff45")
    name_utf16 = "FipSOs FAT32 v45".encode("utf-16-le")[:72]
    struct.pack_into("<16s16sQQQ", entries, 0, type_guid, uniq_guid, part_start, part_end, 0)
    entries[56 : 56 + len(name_utf16)] = name_utf16
    entries_crc = zlib.crc32(entries) & 0xFFFFFFFF

    def build_gpt_header(my_lba: int, alt_lba: int, entry_lba: int) -> bytearray:
        hdr = bytearray(512)
        disk_guid = bytes.fromhex("99887766554433221100aabbccddeeff")
        struct.pack_into(
            "<8sIIIIQQQQ16sQIII",
            hdr,
            0,
            b"EFI PART",
            0x00010000,
            92,
            0,
            0,
            my_lba,
            alt_lba,
            34,
            total_sectors - 34,
            disk_guid,
            entry_lba,
            128,
            128,
            entries_crc,
        )
        hdr_crc = zlib.crc32(hdr[:92]) & 0xFFFFFFFF
        struct.pack_into("<I", hdr, 16, hdr_crc)
        return hdr

    buf[512:1024] = build_gpt_header(1, total_sectors - 1, 2)
    buf[1024 : 1024 + len(entries)] = entries
    buf[(total_sectors - 33) * 512 : (total_sectors - 1) * 512] = entries
    buf[(total_sectors - 1) * 512 : total_sectors * 512] = build_gpt_header(
        total_sectors - 1, 1, total_sectors - 33
    )

    part_off = part_start * 512
    spc = 1
    reserved = 32
    num_fats = 2
    fat_size = 1008

    vbr = bytearray(512)
    vbr[0:3] = b"\xEB\x58\x90"
    vbr[3:11] = b"FIPSOS45"
    struct.pack_into("<H", vbr, 11, 512)
    vbr[13] = spc
    struct.pack_into("<H", vbr, 14, reserved)
    vbr[16] = num_fats
    struct.pack_into("<I", vbr, 28, part_start)
    struct.pack_into("<I", vbr, 32, part_sectors)
    struct.pack_into("<I", vbr, 36, fat_size)
    struct.pack_into("<I", vbr, 44, 2)
    struct.pack_into("<H", vbr, 48, 1)
    struct.pack_into("<H", vbr, 50, 6)
    vbr[64] = 0x80
    vbr[66] = 0x29
    struct.pack_into("<I", vbr, 67, 0x20260450)
    vbr[71:82] = b"FIPSOSDEV45"
    vbr[82:90] = b"FAT32   "
    vbr[510:512] = b"\x55\xAA"
    buf[part_off : part_off + 512] = vbr
    buf[part_off + 6 * 512 : part_off + 7 * 512] = vbr

    data_sectors = part_sectors - reserved - num_fats * fat_size
    fsi = bytearray(512)
    struct.pack_into("<I", fsi, 0, 0x41615252)
    struct.pack_into("<I", fsi, 484, 0x61417272)
    struct.pack_into("<I", fsi, 488, data_sectors - 1)
    struct.pack_into("<I", fsi, 492, 3)
    struct.pack_into("<I", fsi, 508, 0xAA550000)
    buf[part_off + 512 : part_off + 1024] = fsi
    buf[part_off + 7 * 512 : part_off + 8 * 512] = fsi

    fat_sec = bytearray(512)
    struct.pack_into("<I", fat_sec, 0, 0x0FFFFFF8)
    struct.pack_into("<I", fat_sec, 4, 0x0FFFFFFF)
    struct.pack_into("<I", fat_sec, 8, 0x0FFFFFFF)
    fat1_off = part_off + reserved * 512
    fat2_off = fat1_off + fat_size * 512
    buf[fat1_off : fat1_off + 512] = fat_sec
    buf[fat2_off : fat2_off + 512] = fat_sec

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
        "-netdev", "user,id=net0,hostfwd=tcp::2222-:2222",
        "-device", "e1000e,netdev=net0",
        "-boot", "d",
        "-cdrom", ISO_PATH,
    ]
    print("[QEMU] Iniciando FipSOs v4.5...")
    proc = subprocess.Popen(qemu_cmd)
    qmp = None
    try:
        qmp = QmpClient("127.0.0.1", QMP_PORT, timeout=20.0)
        wait_for_serial(SERIAL_LOG, "FipSOs v4.5", 25.0)
        wait_for_serial(SERIAL_LOG, "/mnt/fat0", 25.0)
        wait_for_serial(SERIAL_LOG, "2222", 25.0)
        wait_for_serial(SERIAL_LOG, "fipsos:/#", 25.0)
        print("[PASS] Boot v4.5 + /mnt/fat0 + TCP :2222 + Shell prompt")
        capture_shot(qmp, "shot1_v45_boot")

        run_cmd(qmp, "uname -a", "4.5.0-FipSOs", 10.0)
        print("[PASS] uname -a -> 4.5.0-FipSOs")

        # Create hello.lua on /mnt/fat0
        run_cmd(qmp, 'echo \'print("LUA_TERM_V45_OK")\' > /mnt/fat0/hello.lua', "fipsos:/#", 10.0)

        # 1. Launch desktop_ui.elf and open Integrated Terminal (key '5')
        pos1 = serial_pos(SERIAL_LOG)
        qmp.send_text("desktop_ui.elf", enter=True)
        wait_for_serial(SERIAL_LOG, "Modular Desktop Compositor", 15.0, after_pos=pos1)
        time.sleep(0.5)

        qmp.send_key("5")
        time.sleep(0.35)

        # Execute ls /mnt/fat0 inside Integrated Terminal
        p_ls = serial_pos(SERIAL_LOG)
        qmp.send_text("ls /mnt/fat0", enter=True)
        wait_for_serial(SERIAL_LOG, "[ui_terminal::exec] ls /mnt/fat0", 10.0, after_pos=p_ls)
        time.sleep(0.35)
        capture_shot(qmp, "shot2_v45_term_ls")
        print("[PASS] Terminal integrada ejecuta ls /mnt/fat0 sin cerrar desktop_ui")

        # Execute lua /mnt/fat0/hello.lua inside Integrated Terminal
        p_lua = serial_pos(SERIAL_LOG)
        qmp.send_text("lua /mnt/fat0/hello.lua", enter=True)
        wait_for_serial(SERIAL_LOG, "[ui_terminal::exec] lua /mnt/fat0/hello.lua", 15.0, after_pos=p_lua)
        time.sleep(0.35)
        print("[PASS] Terminal integrada ejecuta /bin/lua sobre /mnt/fat0/hello.lua")

        # Switch theme to Dark Slate and save /mnt/fat0/fipsos.cfg
        p_th = serial_pos(SERIAL_LOG)
        qmp.send_text("theme slate", enter=True)
        wait_for_serial(SERIAL_LOG, "[ui_terminal::exec] theme slate", 10.0, after_pos=p_th)
        time.sleep(0.25)

        p_sv = serial_pos(SERIAL_LOG)
        qmp.send_text("savecfg", enter=True)
        wait_for_serial(SERIAL_LOG, "[desktop_ui] cfg_saved theme=slate exp_dir=/mnt/fat0", 10.0, after_pos=p_sv)
        time.sleep(0.35)
        capture_shot(qmp, "shot3_v45_term_slate")
        print("[PASS] Tema Dark Slate aplicado y guardado en /mnt/fat0/fipsos.cfg")

        # Exit desktop_ui with ESC
        p_ex = serial_pos(SERIAL_LOG)
        qmp.send_key("esc")
        wait_for_serial(SERIAL_LOG, "Retornando a TTY fipsos@v4.5:/$", 15.0, after_pos=p_ex)
        wait_for_serial(SERIAL_LOG, "fipsos:/#", 10.0, after_pos=p_ex)

        # Verify /mnt/fat0/fipsos.cfg contents from shell
        cfg_out = run_cmd(qmp, "cat /mnt/fat0/fipsos.cfg", "theme=slate", 10.0)
        assert "exp_dir=/mnt/fat0" in cfg_out
        print("[PASS] /mnt/fat0/fipsos.cfg persistido correctamente en FAT32")

        # 2. Relaunch desktop_ui.elf and verify persistent cfg_loaded theme=slate
        pos2 = serial_pos(SERIAL_LOG)
        qmp.send_text("desktop_ui.elf", enter=True)
        wait_for_serial(SERIAL_LOG, "[desktop_ui] cfg_loaded theme=slate exp_dir=/mnt/fat0", 15.0, after_pos=pos2)
        time.sleep(0.5)
        capture_shot(qmp, "shot4_v45_persisted_slate")
        print("[PASS] desktop_ui recarga automaticamente theme=slate desde /mnt/fat0/fipsos.cfg")

        p_ex2 = serial_pos(SERIAL_LOG)
        qmp.send_key("esc")
        wait_for_serial(SERIAL_LOG, "Retornando a TTY fipsos@v4.5:/$", 15.0, after_pos=p_ex2)
        print("[ALL PASS] FipSOs v4.5 Desktop Terminal & Persistence Suite completada!")
        return 0
    finally:
        if qmp is not None:
            qmp.quit()
        proc.terminate()
        try:
            proc.wait(timeout=5)
        except Exception:
            proc.kill()


if __name__ == "__main__":
    sys.exit(main())
