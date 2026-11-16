#!/usr/bin/env python3
"""
FipSOs - Fase v4.1 Automated Test Suite (test_v41_native_dev.py)
Validates:
1. FipSOs v4.1 Banner Sync (Kernel, Console, Shell, MOTD, Remote TCP :2222).
2. Native Ring 3 Lua 5.4 Interpreter (/bin/lua):
   - Interactive REPL evaluation (=21*2 -> 42, .exit).
   - Executing /mnt/fat0/test.lua (closures, tables, loops, os.clock, io.open write on FAT32).
3. Native Ring 3 Live C11 Compiler (/bin/tcc + /usr/include + /usr/lib):
   - Writing /mnt/fat0/hello_fips.c directly inside FipSOs.
   - Compiling `tcc /mnt/fat0/hello_fips.c -o /mnt/fat0/hello_fips` to static ELF64 on FAT32.
   - Executing `/mnt/fat0/hello_fips` and verifying stdout + exit status 0.
"""

import json
import os
import re
import socket
import struct
import subprocess
import time
import zlib

SCRATCH_DIR = "/mnt/c/Users/Ferna/.gemini/antigravity/brain/4ef4b43d-91a0-44a4-aefe-f8b38846bd63/scratch"
ARTIFACT_DIR = "/mnt/c/Users/Ferna/.gemini/antigravity/brain/4ef4b43d-91a0-44a4-aefe-f8b38846bd63"
ISO_IMG = "/mnt/c/FipsSOs/build/fipsos-x86_64.iso"
GPT_FAT32_IMG = "/mnt/c/FipsSOs/build/fipsos_gpt_fat32_v41.img"

ANSI_ESCAPE_RE = re.compile(r"\x1b\[[0-9;]*[a-zA-Z]")


def strip_ansi(s: str) -> str:
    return ANSI_ESCAPE_RE.sub("", s)


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
    KEY_MAP = {
        " ": "spc",
        "\n": "ret",
        "-": "minus",
        ".": "dot",
        ",": "comma",
        "/": "slash",
        "\\": "backslash",
        ";": "semicolon",
        "'": "apostrophe",
        "=": "equal",
        "[": "bracket_left",
        "]": "bracket_right",
        "_": "shift-minus",
        ">": "shift-dot",
        "<": "shift-comma",
        "?": "shift-slash",
        "|": "shift-backslash",
        ":": "shift-semicolon",
        '"': "shift-apostrophe",
        "+": "shift-equal",
        "{": "shift-bracket_left",
        "}": "shift-bracket_right",
        "!": "shift-1",
        "#": "shift-3",
        "%": "shift-5",
        "*": "shift-8",
        "(": "shift-9",
        ")": "shift-0",
    }

    def __init__(self, sock_path: str):
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        for _ in range(60):
            try:
                self.sock.connect(sock_path)
                break
            except Exception:
                time.sleep(0.1)
        else:
            raise RuntimeError(f"No se pudo conectar al socket QMP {sock_path}")
        self._read_json()
        self._send_json({"execute": "qmp_capabilities"})
        resp = self._read_json()
        if "return" not in resp:
            raise RuntimeError(f"Error negociando capacidades QMP: {resp}")

    def _read_json(self):
        buf = b""
        while True:
            chunk = self.sock.recv(4096)
            if not chunk:
                break
            buf += chunk
            if b"\r\n" in buf:
                for line in buf.split(b"\r\n"):
                    line = line.strip()
                    if line:
                        try:
                            return json.loads(line.decode("utf-8"))
                        except Exception:
                            continue
        return {}

    def _send_json(self, cmd):
        self.sock.sendall((json.dumps(cmd) + "\r\n").encode("utf-8"))

    def send_key(self, key_name: str):
        self._send_json(
            {"execute": "send-key", "arguments": {"keys": [{"type": "qcode", "data": key_name}]}}
        )
        self._read_json()

    def type_string(self, text: str, delay: float = 0.045):
        for idx, ch in enumerate(text):
            if ch in self.KEY_MAP:
                qcode = self.KEY_MAP[ch]
                if "-" in qcode:
                    parts = qcode.split("-")
                    self._send_json(
                        {
                            "execute": "send-key",
                            "arguments": {"keys": [{"type": "qcode", "data": p} for p in parts]},
                        }
                    )
                    self._read_json()
                else:
                    self.send_key(qcode)
            elif ch.isdigit():
                self.send_key(ch)
            elif ch.isupper():
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
            elif ch.isalpha():
                self.send_key(ch.lower())
            time.sleep(delay)
            if (idx + 1) % 32 == 0:
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
    uniq_guid = bytes.fromhex("112233445566778899aabbccddeeff41")
    name_utf16 = "FipSOs FAT32 Dev".encode("utf-16-le")[:72]
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
    vbr[3:11] = b"FIPSOS41"
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
    struct.pack_into("<I", vbr, 67, 0x20260410)
    vbr[71:82] = b"FIPSOSDEV41"
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


def main() -> None:
    os.makedirs(SCRATCH_DIR, exist_ok=True)
    serial_log = f"{SCRATCH_DIR}/serial_v41.log"
    qmp_sock = "/tmp/fipsos_v41_qmp.sock"
    for f in (serial_log, qmp_sock):
        if os.path.exists(f):
            os.remove(f)

    create_gpt_fat32_disk(GPT_FAT32_IMG)

    qemu_cmd = [
        "qemu-system-x86_64",
        "-M", "q35",
        "-cpu", "max",
        "-smp", "2",
        "-m", "512M",
        "-no-reboot", "-no-shutdown",
        "-serial", f"file:{serial_log}",
        "-qmp", f"unix:{qmp_sock},server,nowait",
        "-drive", f"id=satadisk,file={GPT_FAT32_IMG},format=raw,if=none",
        "-device", "ide-hd,drive=satadisk,bus=ide.0",
        "-device", "qemu-xhci,id=xhci0",
        "-device", "usb-kbd,bus=xhci0.0",
        "-device", "usb-mouse,bus=xhci0.0",
        "-netdev", "user,id=net0,hostfwd=tcp::2222-:2222",
        "-device", "e1000e,netdev=net0",
        "-boot", "d",
        "-cdrom", ISO_IMG,
    ]
    proc = subprocess.Popen(qemu_cmd)
    qmp = None
    try:
        serial = wait_for_serial(serial_log, "for available commands", 25.0)
        assert "[FipSOs] Framekernel FipSOs v4.1 activo" in serial, "Falta banner kernel v4.1"
        assert "FipSOs v4.1 Framekernel" in serial, "Falta banner shell v4.1"
        assert "[FAT32] Particion GPT" in serial and "montada en /mnt/fat0" in serial, "Falta /mnt/fat0"
        print("[OK] Boot FipSOs v4.1 + /mnt/fat0 verificado.")

        qmp = QmpClient(qmp_sock)
        time.sleep(0.4)
        ppm1 = f"{SCRATCH_DIR}/shot1_v41_boot.ppm"
        qmp.screendump(ppm1)
        ppm_to_png(ppm1, f"{ARTIFACT_DIR}/shot1_v41_boot_banner.png")

        # 1. Inspect tools & sysroot in VFS
        pos = serial_pos(serial_log)
        qmp.type_string("ls /usr/lib\n")
        serial = wait_for_serial(serial_log, "fipsos:/#", 10.0, after_pos=pos)
        assert "libc.a" in serial and "crt1.o" in serial and "libtcc1.a" in serial, "Faltan libs C en /usr/lib"

        # 2. Test Lua REPL
        pos = serial_pos(serial_log)
        qmp.type_string("lua\n")
        wait_for_serial(serial_log, "Lua 5.4.7 (FipSOs v4.1)", 8.0, after_pos=pos)
        pos = serial_pos(serial_log)
        qmp.type_string("=21*2\n")
        wait_for_serial(serial_log, "42", 8.0, after_pos=pos)
        pos = serial_pos(serial_log)
        qmp.type_string(".exit\n")
        wait_for_serial(serial_log, "fipsos:/#", 8.0, after_pos=pos)
        print("[OK] Lua REPL (=21*2 -> 42) OK.")

        # 3. Write /mnt/fat0/test.lua inside FipSOs and execute it
        lua_code = (
            "busybox printf '%s\\n' "
            "'local function acc(a) local s=0 for i=1,#a do s=s+a[i] end return s end' "
            "'local r=acc({10,20,12})' "
            "'local f=assert(io.open(\"/mnt/fat0/lua_out.txt\",\"w\")) f:write(\"LUA_FAT32=\"..r..\"\\n\") f:close()' "
            "'print(\"LUA_SCRIPT_OK sum=\"..r..\" clk=\"..string.format(\"%.2f\",os.clock()))' "
            "> /mnt/fat0/test.lua\n"
        )
        pos = serial_pos(serial_log)
        qmp.type_string(lua_code)
        wait_for_serial(serial_log, "fipsos:/#", 15.0, after_pos=pos)
        pos = serial_pos(serial_log)
        qmp.type_string("lua /mnt/fat0/test.lua\n")
        wait_for_serial(serial_log, "LUA_SCRIPT_OK sum=42", 12.0, after_pos=pos)
        wait_for_serial(serial_log, "fipsos:/#", 8.0, after_pos=pos)
        pos = serial_pos(serial_log)
        qmp.type_string("cat /mnt/fat0/lua_out.txt\n")
        wait_for_serial(serial_log, "LUA_FAT32=42", 8.0, after_pos=pos)
        wait_for_serial(serial_log, "fipsos:/#", 8.0, after_pos=pos)
        time.sleep(0.3)
        ppm2 = f"{SCRATCH_DIR}/shot2_v41_lua.ppm"
        qmp.screendump(ppm2)
        ppm_to_png(ppm2, f"{ARTIFACT_DIR}/shot2_v41_lua_exec.png")
        print("[OK] Lua /mnt/fat0/test.lua + FAT32 io.open/write OK.")

        # 4. Write /mnt/fat0/hello_fips.c inside FipSOs, compile with tcc, execute natively
        c_code = (
            "busybox printf '%s\\n' "
            "'#include <stdio.h>' "
            "'int fib(int n){return n<2?n:fib(n-1)+fib(n-2);}' "
            "'int main(void){printf(\"FIPSOS_TCC_LIVE_OK fib10=%d\\n\",fib(10));return 0;}' "
            "> /mnt/fat0/hello_fips.c\n"
        )
        pos = serial_pos(serial_log)
        qmp.type_string(c_code)
        wait_for_serial(serial_log, "fipsos:/#", 15.0, after_pos=pos)
        pos = serial_pos(serial_log)
        qmp.type_string("tcc /mnt/fat0/hello_fips.c -o /mnt/fat0/hello_fips\n")
        tcc_out = wait_for_serial(serial_log, "fipsos:/#", 25.0, after_pos=pos)
        assert "exit=0" in tcc_out, f"tcc no retorno exit=0:\n{tcc_out}"
        pos = serial_pos(serial_log)
        qmp.type_string("ls /mnt/fat0\n")
        wait_for_serial(serial_log, "hello_fips", 12.0, after_pos=pos)
        wait_for_serial(serial_log, "fipsos:/#", 8.0, after_pos=pos)
        pos = serial_pos(serial_log)
        qmp.type_string("/mnt/fat0/hello_fips\n")
        run_out = wait_for_serial(serial_log, "fipsos:/#", 12.0, after_pos=pos)
        assert "FIPSOS_TCC_LIVE_OK fib10=55" in run_out, f"Falta stdout hello_fips:\n{run_out}"
        assert "exit=0" in run_out, f"hello_fips no retorno exit=0:\n{run_out}"
        time.sleep(0.3)
        ppm3 = f"{SCRATCH_DIR}/shot3_v41_tcc.ppm"
        qmp.screendump(ppm3)
        ppm_to_png(ppm3, f"{ARTIFACT_DIR}/shot3_v41_tcc_compile_run.png")
        print("[OK] tcc /mnt/fat0/hello_fips.c -o /mnt/fat0/hello_fips -> FIPSOS_TCC_LIVE_OK fib10=55 OK.")

        # 5. Verify Remote Shell TCP :2222 v4.1 banner
        s = socket.create_connection(("127.0.0.1", 2222), timeout=5.0)
        banner = s.recv(1024).decode("utf-8", errors="ignore")
        s.close()
        assert "FipSOs v4.1 Remote Shell" in banner, f"Banner remoto inesperado: {banner!r}"
        print("[OK] Remote Shell TCP :2222 v4.1 banner OK.")

        pos = serial_pos(serial_log)
        qmp.type_string("uname -a\n")
        wait_for_serial(serial_log, "4.1.0-FipSOs", 8.0, after_pos=pos)
        ppm4 = f"{SCRATCH_DIR}/shot4_v41_uname.ppm"
        qmp.screendump(ppm4)
        ppm_to_png(ppm4, f"{ARTIFACT_DIR}/shot4_v41_uname.png")

        print("\n[ALL PASS] test_v41_native_dev.py completado con exito!")
    finally:
        if qmp is not None:
            qmp.quit()
        proc.terminate()
        try:
            proc.wait(timeout=5)
        except Exception:
            proc.kill()


if __name__ == "__main__":
    main()
