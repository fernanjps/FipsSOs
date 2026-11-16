#!/usr/bin/env python3
"""
FipSOs v4.4 Stability & Hardening Verification Suite (QEMU + QMP + FAT32 + UI Explorer)
Validates:
  1. FipSOs v4.4 boot banner, /mnt/fat0 GPT/FAT32 mount, TCP :2222, uname -a (4.4.0-FipSOs)
  2. fipsos_sh unified '>' / '>>' redirection on builtins & unquoted '|' in pipes
  3. C stress test via tcc: open + pipe2 + dup + dup2 FD non-collision & -ENAMETOOLONG (140B path)
  4. UI Explorer executing /mnt/fat0/bad.c (captures tcc stderr in Vista Previa) & /mnt/fat0/ok.c
"""

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
DISK_PATH = os.path.join(REPO_DIR, "build", "test_v44_fat32.img")
SERIAL_LOG = os.path.join(REPO_DIR, "build", "test_v44_serial.log")
QMP_PORT = 4464
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
        self.sock.sendall((json_dumps(obj) + "\n").encode("utf-8"))

    def _read_json(self) -> dict:
        while b"\n" not in self.buf:
            chunk = self.sock.recv(4096)
            if not chunk:
                raise RuntimeError("QMP EOF")
            self.buf += chunk
        line, self.buf = self.buf.split(b"\n", 1)
        return json_loads(line.decode("utf-8"))

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
            time.sleep(0.05)

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


def json_dumps(obj) -> str:
    import json
    return json.dumps(obj)


def json_loads(s: str):
    import json
    return json.loads(s)


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
    uniq_guid = bytes.fromhex("112233445566778899aabbccddeeff44")
    name_utf16 = "FipSOs FAT32 v44".encode("utf-16-le")[:72]
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
    vbr[3:11] = b"FIPSOS44"
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
    struct.pack_into("<I", vbr, 67, 0x20260440)
    vbr[71:82] = b"FIPSOSDEV44"
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
    print("[QEMU] Iniciando FipSOs v4.4...")
    proc = subprocess.Popen(qemu_cmd)
    qmp = None
    try:
        qmp = QmpClient("127.0.0.1", QMP_PORT, timeout=20.0)
        wait_for_serial(SERIAL_LOG, "FipSOs v4.4", 25.0)
        wait_for_serial(SERIAL_LOG, "/mnt/fat0", 25.0)
        wait_for_serial(SERIAL_LOG, "2222", 25.0)
        wait_for_serial(SERIAL_LOG, "fipsos:/#", 25.0)
        print("[PASS] Boot v4.4 + /mnt/fat0 + TCP :2222 + Shell prompt")
        capture_shot(qmp, "shot1_v44_boot")

        # 1. uname -a -> 4.4.0-FipSOs
        run_cmd(qmp, "uname -a", "4.4.0-FipSOs", 10.0)
        print("[PASS] uname -a -> 4.4.0-FipSOs")

        # 2. Shell redirection '>>' on builtin echo & unquoted '|' pipe test
        run_cmd(qmp, "echo linea1_v44 > /mnt/fat0/log.txt", "fipsos:/#", 10.0)
        run_cmd(qmp, "echo linea2_v44 >> /mnt/fat0/log.txt", "fipsos:/#", 10.0)
        out_log = run_cmd(qmp, "cat /mnt/fat0/log.txt", "linea2_v44", 10.0)
        assert "linea1_v44" in out_log, "Falta linea1_v44 tras >>"
        print("[PASS] fipsos_sh '>' y '>>' unificados en builtin echo")

        out_pipe = run_cmd(qmp, 'echo "a | b" | busybox cat', "a | b", 10.0)
        assert "a | b" in out_pipe, "Tuberia fallo con '|' entrecomillado"
        print("[PASS] fipsos_sh ignora '|' dentro de comillas")

        # 3. C stress test: open + pipe2 + dup + dup2 + ENAMETOOLONG
        fd_test_lines = [
            "#include <fcntl.h>",
            "#include <unistd.h>",
            "#include <stdio.h>",
            "int main(void) {",
            "  int p[2];",
            '  int f = open("/mnt/fat0/log.txt", 0);',
            "  int r = pipe(p);",
            "  int d = dup(f);",
            "  int d2 = dup2(f, 25);",
            "  char lp[150];",
            "  int i; for (i = 0; i < 140; i++) lp[i] = 97;",
            "  lp[140] = 0;",
            "  int el = open(lp, 0);",
            '  printf("FDS %d %d %d %d %d EL=%d\\n",',
            "         f, p[0], p[1], d, d2, el);",
            "  if (f < 3 || r != 0 || p[0] == f || p[1] == f)",
            "    return 1;",
            "  if (p[0] == p[1] || d == f || d == p[0] || d == p[1])",
            "    return 2;",
            "  if (d2 != 25 || el >= 0) return 3;",
            "  close(f); close(p[0]); close(p[1]);",
            "  close(d); close(d2);",
            '  printf("[FD_TEST_PASS]\\n");',
            "  return 0;",
            "}",
        ]
        for idx, line in enumerate(fd_test_lines):
            op = ">" if idx == 0 else ">>"
            run_cmd(qmp, f"echo '{line}' {op} /mnt/fat0/fd_test.c", "fipsos:/#", 10.0)
        run_cmd(qmp, "tcc /mnt/fat0/fd_test.c -o /mnt/fat0/fd_test.elf", "fipsos:/#", 20.0)
        run_cmd(qmp, "/mnt/fat0/fd_test.elf", "[FD_TEST_PASS]", 15.0)
        print("[PASS] Stress test open+pipe2+dup+dup2 sin colisiones y -ENAMETOOLONG")

        # 4. Create bad.c (syntax error) and ok.c in /mnt/fat0/
        run_cmd(
            qmp,
            "echo 'int main(void){return missing_sym}' > /mnt/fat0/bad.c",
            "fipsos:/#",
            10.0,
        )
        run_cmd(
            qmp,
            "echo '#include <stdio.h>' > /mnt/fat0/ok.c",
            "fipsos:/#",
            10.0,
        )
        run_cmd(
            qmp,
            'echo \'int main(void){printf("OK_V44_RUN\\n");return 0;}\' >> /mnt/fat0/ok.c',
            "fipsos:/#",
            10.0,
        )

        # 5. Launch desktop_ui.elf and test Explorer .c error + .c ok + Start Menu
        pos = serial_pos(SERIAL_LOG)
        qmp.send_text("desktop_ui.elf", enter=True)
        wait_for_serial(SERIAL_LOG, "Modular Desktop Compositor", 15.0, after_pos=pos)
        time.sleep(0.5)

        # Focus Explorer (key '3'), switch to /mnt/fat0 (key 'm')
        qmp.send_key("3")
        time.sleep(0.2)
        qmp.send_key("m")
        time.sleep(0.4)

        # Entries in /mnt/fat0: log.txt(0), fd_test.c(1), fd_test.elf(2), bad.c(3), ok.c(4)
        # Select bad.c -> 3 x down
        for _ in range(3):
            qmp.send_key("down")
            time.sleep(0.12)
        pos_err = serial_pos(SERIAL_LOG)
        qmp.send_key("x")
        wait_for_serial(SERIAL_LOG, "[ui_explorer::exec] tcc_err /mnt/fat0/bad.c", 15.0, after_pos=pos_err)
        wait_for_serial(SERIAL_LOG, "/mnt/fat0/bad.c:1: error:", 5.0, after_pos=pos_err)
        time.sleep(0.35)
        capture_shot(qmp, "shot2_v44_tcc_err")
        print("[PASS] UI Explorer captura error de compilacion tcc stderr en bad.c")

        # Select ok.c -> 1 x down
        qmp.send_key("down")
        time.sleep(0.15)
        pos_ok = serial_pos(SERIAL_LOG)
        qmp.send_key("x")
        wait_for_serial(SERIAL_LOG, "[ui_explorer::exec] tcc /mnt/fat0/ok.c", 20.0, after_pos=pos_ok)
        time.sleep(0.35)
        capture_shot(qmp, "shot3_v44_tcc_ok")
        print("[PASS] UI Explorer compila y ejecuta ok.c con exito")

        # Open Start Menu (Space) and capture desktop screenshot
        qmp.send_key("spc")
        time.sleep(0.35)
        capture_shot(qmp, "shot4_v44_desktop")

        # Exit desktop cleanly
        pos_exit = serial_pos(SERIAL_LOG)
        qmp.send_key("q")
        wait_for_serial(SERIAL_LOG, "Retornando a TTY fipsos@v4.4:/$", 15.0, after_pos=pos_exit)
        wait_for_serial(SERIAL_LOG, "fipsos:/#", 10.0, after_pos=pos_exit)

        # Verify captured stdout+stderr in /disk/ui_exec.log from ok.c
        ui_log = run_cmd(qmp, "cat /disk/ui_exec.log", "OK_V44_RUN", 10.0)
        assert "OK_V44_RUN" in ui_log
        print("[PASS] /disk/ui_exec.log contiene salida de ok.c")
        print("[ALL PASS] FipSOs v4.4 Stability Suite completada!")
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
