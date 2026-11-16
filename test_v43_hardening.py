#!/usr/bin/env python3
"""
FipSOs - Fase v4.3 Automated Test Suite (test_v43_hardening.py)
Validates:
1. FipSOs v4.3 Banner Sync (Kernel, Console, Shell, MOTD, Remote TCP :2222, uname -a 4.3.0-FipSOs).
2. Kernel VFS / Syscalls hardening compiled live with /bin/tcc:
   - open() and openat() with >127-byte path return -1 with errno == 36 (ENAMETOOLONG).
   - Per-process getdents64 directory cursor isolation across fork() via ProcessFdTable.
3. Modular C++20 Desktop UI (`desktop_ui.elf` via `gui`):
   - Direct execution of /mnt/fat0/demo.lua (via /bin/lua) from UI Explorer with live preview capture.
   - Direct live compile + run of /mnt/fat0/hola_fat.c (via /bin/tcc) from UI Explorer with preview capture.
   - Switching to /bin tab via USB HID mouse click (xHCI) + full >20 entry directory listing + Scroll pagination (`Pag. Abajo`).
   - Start Menu tile 4 launching /bin/avatar_cpp.elf (instead of c_std_app.elf) and clean ESC exit to TTY.
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
GPT_FAT32_IMG = "/mnt/c/FipsSOs/build/fipsos_gpt_fat32_v43.img"

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
        "@": "shift-2",
        "#": "shift-3",
        "%": "shift-5",
        "&": "shift-7",
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

    def type_string(self, text: str, delay: float = 0.035):
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
                time.sleep(0.06)

    def send_mouse_rel(self, dx: int, dy: int):
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
            self._send_json({"execute": "input-send-event", "arguments": {"events": events}})
            self._read_json()
            time.sleep(0.04)

    def reset_mouse_topleft(self):
        for _ in range(18):
            self.send_mouse_rel(-80, -80)
            time.sleep(0.025)

    def click_at(self, tx: int, ty: int):
        self.reset_mouse_topleft()
        self.send_mouse_rel(tx, ty)
        time.sleep(0.08)
        self._send_json(
            {
                "execute": "input-send-event",
                "arguments": {"events": [{"type": "btn", "data": {"button": "left", "down": True}}]},
            }
        )
        self._read_json()
        time.sleep(0.08)
        self._send_json(
            {
                "execute": "input-send-event",
                "arguments": {"events": [{"type": "btn", "data": {"button": "left", "down": False}}]},
            }
        )
        self._read_json()
        time.sleep(0.15)

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
    uniq_guid = bytes.fromhex("112233445566778899aabbccddeeff43")
    name_utf16 = "FipSOs FAT32 v43".encode("utf-16-le")[:72]
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
    vbr[3:11] = b"FIPSOS43"
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
    struct.pack_into("<I", vbr, 67, 0x20260430)
    vbr[71:82] = b"FIPSOSDEV43"
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
    serial_log = f"{SCRATCH_DIR}/serial_v43.log"
    qmp_sock = "/tmp/fipsos_v43_qmp.sock"
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
        assert "[FipSOs] Framekernel FipSOs v4.3 activo" in serial, "Falta banner kernel v4.3"
        assert "FipSOs v4.3 Framekernel" in serial, "Falta banner shell v4.3"
        assert "[FAT32] Particion GPT" in serial and "montada en /mnt/fat0" in serial, "Falta /mnt/fat0"
        print("[OK] Boot FipSOs v4.3 + /mnt/fat0 verificado.")

        qmp = QmpClient(qmp_sock)
        time.sleep(0.4)
        ppm1 = f"{SCRATCH_DIR}/shot1_v43_boot.ppm"
        qmp.screendump(ppm1)
        ppm_to_png(ppm1, f"{ARTIFACT_DIR}/shot1_v43_boot_banner.png")

        # 1. Remote Shell TCP :2222 v4.3 + uname -a
        s = socket.create_connection(("127.0.0.1", 2222), timeout=5.0)
        banner = s.recv(1024).decode("utf-8", errors="ignore")
        s.close()
        assert "FipSOs v4.3 Remote Shell" in banner, f"Banner remoto inesperado: {banner!r}"
        print("[OK] Remote Shell TCP :2222 v4.3 banner OK.")

        pos = serial_pos(serial_log)
        qmp.type_string("uname -a\n")
        uname_out = wait_for_serial(serial_log, "fipsos:/#", 8.0, after_pos=pos)
        assert "4.3.0-FipSOs" in uname_out, f"uname -a no muestra 4.3.0-FipSOs:\n{uname_out}"
        print("[OK] uname -a -> 4.3.0-FipSOs OK.")

        # 2. Programa C en /disk para validar -ENAMETOOLONG (-36) y cursores getdents64 por proceso
        cmds_c = [
            "busybox printf '%s\\n' '#include <errno.h>' '#include <fcntl.h>' '#include <stdio.h>' '#include <string.h>' '#include <unistd.h>' > /disk/v43_test.c\n",
            "busybox printf '%s\\n' 'extern long __fips_syscall(long,long,long,long,long,long,long);' 'int main(void){char b[144];memset(b,97,140);b[0]=47;b[140]=0;' >> /disk/v43_test.c\n",
            "busybox printf '%s\\n' 'errno=0;int r1=open(b,0);int e1=errno;long r2=__fips_syscall(257,-100,(long)b,0,0,0,0);' 'int d=open(\"/bin\",0);char u[512];' >> /disk/v43_test.c\n",
            "busybox printf '%s\\n' 'long n0=__fips_syscall(217,d,(long)u,48,0,0,0);if(fork()==0){__fips_syscall(217,d,(long)u,512,0,0,0);__fips_syscall(217,d,(long)u,512,0,0,0);_exit(0);}' >> /disk/v43_test.c\n",
            "busybox printf '%s\\n' 'int s=0;__fips_syscall(61,-1,(long)&s,0,0,0,0);long n1=__fips_syscall(217,d,(long)u,512,0,0,0);close(d);' >> /disk/v43_test.c\n",
            "busybox printf '%s\\n' 'printf(\"V43_SYS r1=%d e1=%d r2=%ld n0=%ld n1=%ld\\n\",r1,e1,r2,n0,n1);return 0;}' >> /disk/v43_test.c\n",
        ]
        for cmd in cmds_c:
            pos = serial_pos(serial_log)
            qmp.type_string(cmd)
            wait_for_serial(serial_log, "fipsos:/#", 15.0, after_pos=pos)

        pos = serial_pos(serial_log)
        qmp.type_string("tcc /disk/v43_test.c -o /disk/v43_test.elf\n")
        wait_for_serial(serial_log, "fipsos:/#", 15.0, after_pos=pos)

        pos = serial_pos(serial_log)
        qmp.type_string("/disk/v43_test.elf\n")
        sys_out = wait_for_serial(serial_log, "fipsos:/#", 15.0, after_pos=pos)
        m_sys = re.search(r"V43_SYS r1=(-?\d+) e1=(\d+) r2=(-?\d+) n0=(\d+) n1=(\d+)", sys_out)
        assert m_sys is not None, f"Falto salida V43_SYS:\n{sys_out}"
        r1, e1, r2, n0, n1 = map(int, m_sys.groups())
        assert r1 == -1 and e1 == 36 and r2 == -36, f"Fallo -ENAMETOOLONG: r1={r1}, e1={e1}, r2={r2}"
        assert n0 > 0 and n1 > 200, f"Fallo aislamiento cursor getdents64: n0={n0}, n1={n1}"
        print(f"[OK] -ENAMETOOLONG (-36) y cursor getdents64 aislado (n0={n0}, n1={n1}) verificados.")

        # 3. Crear demo.lua y hola_fat.c en /mnt/fat0 para pruebas integradas del Explorador Gráfico
        pos = serial_pos(serial_log)
        qmp.type_string("busybox printf '%s\\n' 'print(\"LUA_UI_V43_OK 42\")' 'print(\"FipSOs FAT32 Lua Engine\")' > /mnt/fat0/demo.lua\n")
        wait_for_serial(serial_log, "fipsos:/#", 15.0, after_pos=pos)

        pos = serial_pos(serial_log)
        qmp.type_string("busybox printf '%s\\n' '#include <stdio.h>' 'int main(void){printf(\"TCC_UI_V43_OK 43\\n\");return 0;}' > /mnt/fat0/hola_fat.c\n")
        wait_for_serial(serial_log, "fipsos:/#", 15.0, after_pos=pos)

        # 4. Lanzar desktop_ui (`gui`)
        pos = serial_pos(serial_log)
        qmp.type_string("gui\n")
        gui_out = wait_for_serial(serial_log, "[ui_explorer::preview]", 15.0, after_pos=pos)
        assert "FipSOs v4.3 Modular Desktop Compositor" in gui_out, f"Falta banner desktop_ui v4.3:\n{gui_out}"
        assert "demo.lua" in gui_out and "hola_fat.c" in gui_out, f"Faltan archivos FAT32 en Explorer:\n{gui_out}"

        # Seleccionar y ejecutar demo.lua con ENTER (`ret` o `'x'`)
        pos_exec = serial_pos(serial_log)
        qmp.send_key("x")
        lua_exec_out = wait_for_serial(serial_log, "[ui_explorer::exec]", 15.0, after_pos=pos_exec)
        assert "lua /mnt/fat0/demo.lua -> LUA_UI_V43_OK 42" in lua_exec_out, f"Fallo ejecucion Lua desde UI:\n{lua_exec_out}"
        time.sleep(0.5)
        ppm2 = f"{SCRATCH_DIR}/shot2_v43_lua_exec.ppm"
        qmp.screendump(ppm2)
        ppm_to_png(ppm2, f"{ARTIFACT_DIR}/shot2_v43_lua_exec.png")
        print("[OK] Ejecucion directa de /mnt/fat0/demo.lua desde UI Explorer con Vista Previa OK.")

        # Ciclar a hola_fat.c (`f`) y compilar+ejecutar con `ret` (Enter)
        pos_c = serial_pos(serial_log)
        qmp.send_key("f")
        wait_for_serial(serial_log, "[ui_explorer::preview] /mnt/fat0/hola_fat.c", 10.0, after_pos=pos_c)
        pos_c_exec = serial_pos(serial_log)
        qmp.send_key("ret")
        tcc_exec_out = wait_for_serial(serial_log, "[ui_explorer::exec]", 20.0, after_pos=pos_c_exec)
        assert "tcc /mnt/fat0/hola_fat.c -> TCC_UI_V43_OK 43" in tcc_exec_out, f"Fallo compilacion/ejecucion TCC desde UI:\n{tcc_exec_out}"
        time.sleep(0.5)
        ppm3 = f"{SCRATCH_DIR}/shot3_v43_tcc_exec.ppm"
        qmp.screendump(ppm3)
        ppm_to_png(ppm3, f"{ARTIFACT_DIR}/shot3_v43_tcc_exec.png")
        print("[OK] Compilacion y ejecucion directa de /mnt/fat0/hola_fat.c (tcc) desde UI Explorer OK.")

        # 5. Clic real con raton en pestana /bin (x=400, y=435) para verificar listado >12 entradas + Paginacion Scroll
        pos_bin = serial_pos(serial_log)
        qmp.click_at(400, 435)
        bin_out = wait_for_serial(serial_log, "[ui_explorer] dir=/bin", 12.0, after_pos=pos_bin)
        m = re.search(r"dir=/bin entries=(\d+):", bin_out)
        assert m is not None and int(m.group(1)) >= 20, f"Listado /bin truncado o inesperado:\n{bin_out}"
        print(f"[OK] Pestana /bin por raton xHCI cargo {m.group(1)} entradas reales (sin limite de 12).")

        pos_scr = serial_pos(serial_log)
        qmp.click_at(290, 705)  # Boton "Pag. Abajo" (x=258..370, y=692..718)
        scr_out = wait_for_serial(serial_log, "[ui_explorer::scroll]", 10.0, after_pos=pos_scr)
        assert "dir=/bin scroll=9" in scr_out, f"Fallo scroll paginado en /bin:\n{scr_out}"
        time.sleep(0.4)
        ppm4 = f"{SCRATCH_DIR}/shot4_v43_bin_paginated.ppm"
        qmp.screendump(ppm4)
        ppm_to_png(ppm4, f"{ARTIFACT_DIR}/shot4_v43_bin_paginated.png")
        print("[OK] Paginacion vertical real (scroll=9) en /bin verificada.")

        # 6. Abrir Menu Inicio ('spc') y lanzar mosaico 4 `avatar_cpp` con clic en (640, 612)
        qmp.send_key("spc")
        time.sleep(0.4)
        pos_av = serial_pos(serial_log)
        qmp.click_at(640, 612)
        wait_for_serial(serial_log, "/bin/avatar_cpp.elf", 15.0, after_pos=pos_av)
        time.sleep(0.6)
        qmp.send_key("q")
        av_out = wait_for_serial(serial_log, "[avatar_cpp.elf] Resumen", 15.0, after_pos=pos_av)
        print("[OK] Mosaico 4 del Menu Inicio lanzo /bin/avatar_cpp.elf y regreso limpio al desktop.")

        # Salir del desktop con ESC y verificar retorno limpio a TTY
        time.sleep(0.4)
        pos_esc = serial_pos(serial_log)
        qmp.send_key("esc")
        esc_out = wait_for_serial(serial_log, "fipsos:/#", 12.0, after_pos=pos_esc)
        assert "Sesion grafica finalizada limpiamente. Retornando a TTY fipsos@v4.3:/$" in esc_out, f"Falta cierre limpio ESC v4.3:\n{esc_out}"
        print("[OK] Salida limpia con tecla ESC restaurando TTY fipsos@v4.3:/# OK.")

        print("\n[ALL PASS] test_v43_hardening.py completado con exito!")
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
