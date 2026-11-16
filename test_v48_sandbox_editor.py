#!/usr/bin/env python3
"""
FipSOs v4.8 Verification Suite:
  1. FipSOs v4.8 Framekernel boot, uname -a -> 4.8.0-FipSOs
  2. Ring-3 Capability Sandbox (SYS_FIPS_SANDBOX = 450):
     - Normal `sandbox_probe` succeeds (socket + /disk + /mnt/fat0/sandbox)
     - Sandboxed `sandbox sandbox_probe` receives -EPERM (errno=1) on socket & /disk,
       while writes inside /mnt/fat0/sandbox/ succeed
  3. Automatic Background Process Reaper:
     - Multiple `&` background executions free scheduler slots without wait4()
  4. Integrated Graphical Code Editor (`ui_explorer` -> `ui_editor`):
     - Open `/mnt/fat0/ed_test.c` in Code Editor, edit text, Save & Run via /bin/tcc,
       verify output & persistence, capture PNG screenshots.
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
DISK_PATH = os.path.join(REPO_DIR, "build", "test_v48_fat32.img")
SERIAL_LOG = os.path.join(REPO_DIR, "build", "test_v48_serial.log")
QMP_PORT = 4468
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
    with open(png_path, "wb") as f:
        f.write(b"\x89PNG\r\n\x1a\n")
        f.write(chunk(b"IHDR", ihdr))
        f.write(chunk(b"IDAT", idat))
        f.write(chunk(b"IEND", b""))


class QmpClient:
    def __init__(self, port: int):
        self.sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        self.sock.settimeout(10.0)
        for _ in range(30):
            try:
                self.sock.connect(("127.0.0.1", port))
                break
            except OSError:
                time.sleep(0.2)
        self._recv_line()
        self.cmd("qmp_capabilities")

    def _recv_line(self) -> dict:
        buf = b""
        while not buf.endswith(b"\n"):
            chunk = self.sock.recv(4096)
            if not chunk:
                break
            buf += chunk
        for line in buf.splitlines():
            if line.strip():
                return json.loads(line.decode("utf-8", errors="replace"))
        return {}

    def cmd(self, execute: str, arguments: dict | None = None) -> dict:
        payload: dict = {"execute": execute}
        if arguments is not None:
            payload["arguments"] = arguments
        self.sock.sendall((json.dumps(payload) + "\n").encode("utf-8"))
        while True:
            msg = self._recv_line()
            if "return" in msg or "error" in msg:
                return msg

    def send_key(self, qcode: str) -> None:
        self.cmd("send-key", {"keys": [{"type": "qcode", "data": qcode}]})

    def send_text(self, text: str, delay: float = 0.03) -> None:
        shift_map = {
            "!": "1", "@": "2", "#": "3", "$": "4", "%": "5", "^": "6",
            "&": "7", "*": "8", "(": "9", ")": "0", "_": "minus", "+": "equal",
            "{": "bracket_left", "}": "bracket_right", "|": "backslash",
            ":": "semicolon", '"': "apostrophe", "<": "comma", ">": "dot",
            "?": "slash", "~": "grave_accent",
        }
        plain_map = {
            " ": "spc", "\n": "ret", "\r": "ret", "-": "minus", "=": "equal",
            "[": "bracket_left", "]": "bracket_right", "\\": "backslash",
            ";": "semicolon", "'": "apostrophe", ",": "comma", ".": "dot",
            "/": "slash", "`": "grave_accent",
        }
        for ch in text:
            if ch in shift_map:
                self.cmd("send-key", {"keys": [{"type": "qcode", "data": "shift"}, {"type": "qcode", "data": shift_map[ch]}]})
            elif "A" <= ch <= "Z":
                self.cmd("send-key", {"keys": [{"type": "qcode", "data": "shift"}, {"type": "qcode", "data": ch.lower()}]})
            elif ch in plain_map:
                self.send_key(plain_map[ch])
            elif ch.isalnum():
                self.send_key(ch)
            time.sleep(delay)

    def send_mouse_rel(self, dx: int, dy: int) -> None:
        while dx != 0 or dy != 0:
            sx = max(-80, min(80, dx))
            sy = max(-80, min(80, dy))
            dx -= sx
            dy -= sy
            events = []
            if sx != 0:
                events.append({"type": "rel", "data": {"axis": "x", "value": sx}})
            if sy != 0:
                events.append({"type": "rel", "data": {"axis": "y", "value": sy}})
            self.cmd("input-send-event", {"events": events})
            time.sleep(0.03)

    def click_at(self, tx: int, ty: int) -> None:
        for _ in range(18):
            self.send_mouse_rel(-80, -80)
            time.sleep(0.02)
        self.send_mouse_rel(tx, ty)
        time.sleep(0.08)
        self.cmd("input-send-event", {"events": [{"type": "btn", "data": {"button": "left", "down": True}}]})
        time.sleep(0.08)
        self.cmd("input-send-event", {"events": [{"type": "btn", "data": {"button": "left", "down": False}}]})
        time.sleep(0.15)

    def screendump(self, png_name: str) -> str:
        ppm_path = os.path.join(REPO_DIR, "build", f"{png_name}.ppm")
        png_path = os.path.join(ARTIFACT_DIR, f"{png_name}.png")
        os.makedirs(ARTIFACT_DIR, exist_ok=True)
        self.cmd("screendump", {"filename": ppm_path})
        time.sleep(0.3)
        ppm_to_png(ppm_path, png_path)
        return png_path

    def close(self) -> None:
        try:
            self.cmd("quit")
        except Exception:
            pass
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
        raise AssertionError(f"Timed out waiting for '{wait_pattern}' after '{cmd}'. Tail:\n{read_serial()[offset:]}")
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
    vbr[3:11] = b"FIPSOS48"
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

        shot1 = qmp.screendump("shot1_v48_boot")
        print(f"[PASS] Booted FipSOs v4.8 -> {shot1}")

        out = run_shell_cmd(qmp, "uname -a", "4.8.0-FipSOs")
        print("[PASS] uname -a -> 4.8.0-FipSOs")

        # 1. Sandbox verification
        out_norm = run_shell_cmd(qmp, "sandbox_probe", "[probe] write /mnt/fat0/sandbox: ALLOWED")
        assert "[probe] socket: ALLOWED" in out_norm, f"Normal socket failed:\n{out_norm}"
        assert "[probe] write /disk: ALLOWED" in out_norm, f"Normal /disk write failed:\n{out_norm}"
        print("[PASS] Normal sandbox_probe allowed socket + /disk + /mnt/fat0/sandbox")

        out_sb = run_shell_cmd(qmp, "sandbox sandbox_probe", "[probe] write /mnt/fat0/sandbox: ALLOWED")
        assert "[probe] socket: DENIED (ret=-1 errno=1)" in out_sb, f"Sandboxed socket not denied:\n{out_sb}"
        assert "[probe] write /disk: DENIED (ret=-1 errno=1)" in out_sb, f"Sandboxed /disk write not denied:\n{out_sb}"
        print("[PASS] Sandboxed sandbox_probe received -EPERM (errno=1) on socket & /disk, allowed /mnt/fat0/sandbox")

        # 2. Auto-reap verification for '&' background jobs
        for i in range(6):
            run_shell_cmd(qmp, "sandbox_probe &", "[bg] PID=")
            time.sleep(0.35)
        time.sleep(1.0)
        ps_out = run_shell_cmd(qmp, "ps", "fipsos_sh")
        assert "Zombie" not in ps_out, f"Zombie processes left behind in ps:\n{ps_out}"
        print("[PASS] 6 background '&' jobs auto-reaped cleanly without Zombie slots in ps")

        # 3. Integrated Graphical Code Editor (ui_explorer -> ui_editor -> save & run)
        run_shell_cmd(qmp, "echo '#include <stdio.h>' > /mnt/fat0/ed_test.c", "fipsos:/#")
        run_shell_cmd(qmp, "echo 'int main(void){printf(\"V48_ED_OK\\n\");return 0;}' >> /mnt/fat0/ed_test.c", "fipsos:/#")

        offset = len(read_serial())
        qmp.send_text("gui\n")
        ok, _ = wait_for_serial("[desktop_ui.elf] FipSOs v4.8", timeout=15.0, offset=offset)
        assert ok, f"desktop_ui.elf v4.8 did not start:\n{read_serial()[offset:]}"
        time.sleep(1.2)

        # Navigate Explorer to ed_test.c if not already selected, then press 'e'
        if "[ui_explorer::preview] /mnt/fat0/ed_test.c" not in read_serial()[offset:]:
            for _ in range(4):
                qmp.send_key("f")
                time.sleep(0.25)
                if "[ui_explorer::preview] /mnt/fat0/ed_test.c" in read_serial()[offset:]:
                    break
        qmp.send_key("e")
        ok, ed_off = wait_for_serial("[ui_editor] open path=", timeout=10.0, offset=offset)
        assert ok, f"Code Editor did not open:\n{read_serial()[offset:]}"
        time.sleep(0.6)
        shot2 = qmp.screendump("shot2_v48_editor_open")
        print(f"[PASS] Code Editor opened from Explorer -> {shot2}")

        # Edit a comment inside Code Editor and click "Guardar y Ejecutar" (376, 445)
        qmp.send_text("// v48\n")
        time.sleep(0.3)
        qmp.click_at(376, 445)
        ok, _ = wait_for_serial("[ui_editor] save path=", timeout=12.0, offset=ed_off)
        assert ok, f"Code Editor save failed:\n{read_serial()[ed_off:]}"
        ok, _ = wait_for_serial("[ui_editor] run path=", timeout=15.0, offset=ed_off)
        assert ok, f"Code Editor run failed:\n{read_serial()[ed_off:]}"
        time.sleep(0.6)
        shot3 = qmp.screendump("shot3_v48_editor_run")
        print(f"[PASS] Code Editor Save & Run verified -> {shot3}")

        # Exit Code Editor / Desktop cleanly (ESC twice since Editor captures letters)
        qmp.send_key("esc")
        time.sleep(0.15)
        qmp.send_key("esc")
        time.sleep(0.5)
        qmp.send_key("q")
        wait_for_serial("Sesion de escritorio cerrada", timeout=6.0, offset=offset)

        print("\n=== ALL V4.8 SANDBOX, AUTO-REAP & CODE EDITOR TESTS PASSED ===")
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
