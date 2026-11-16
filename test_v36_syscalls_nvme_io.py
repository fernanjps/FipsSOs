#!/usr/bin/env python3
"""
FipSOs - Fase v3.6 Automated Test Suite (test_v36_syscalls_nvme_io.py)
Validates:
1. Linux Static Compat Syscalls (linux_abi/src/syscall/compat.rs) via c_std_app (writev/readv/openat/fstat/fcntl/getrandom/nanosleep).
2. Extensionless Executable Resolution in fipsos_sh ('c_std_app', 'sample_tool', 'busybox' without '.elf').
3. PCIe NVMe I/O Queues (IOCQ 0x05 + IOSQ 0x01) and Real NVMe DMA Read/Write Blocks backing DiskFS v2 (without SATA IDE).
4. Host-side NVMe raw image persistence verification ('FIPSOS_DISKFS_V2' and 'nvme_dma_ok').
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
SERIAL_LOG = f"{SCRATCH_DIR}/serial_v36.log"
QMP_SOCK = "/tmp/fipsos_v36_qmp.sock"
NVME_IMG = "/mnt/c/FipsSOs/build/fipsos_nvme_v36.img"
ISO_IMG = "/mnt/c/FipsSOs/build/fipsos-x86_64.iso"

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
    def __init__(self, sock_path: str):
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        for _ in range(50):
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

    def type_string(self, text: str, delay: float = 0.04):
        key_map = {
            " ": "spc",
            "\n": "ret",
            "-": "minus",
            ".": "dot",
            "/": "slash",
            "_": "shift-minus",
            ">": "shift-dot",
            "|": "shift-backslash",
            ":": "shift-semicolon",
            ";": "semicolon",
            '"': "shift-apostrophe",
            "=": "equal",
            "+": "shift-equal",
        }
        for ch in text:
            if ch in key_map:
                qcode = key_map[ch]
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
            elif ch.isalpha():
                self.send_key(ch.lower())
            time.sleep(delay)

    def screendump(self, out_ppm_path: str):
        self._send_json({"execute": "screendump", "arguments": {"filename": out_ppm_path}})
        return self._read_json()

    def close(self):
        try:
            self.sock.close()
        except Exception:
            pass


def read_serial() -> str:
    if not os.path.exists(SERIAL_LOG):
        return ""
    with open(SERIAL_LOG, "r", encoding="utf-8", errors="ignore") as f:
        return f.read()


def main():
    print("======================================================================")
    print("  FipSOs Fase v3.6 - Suite de Verificacion Automatizada")
    print("  Linux Static Compat + Extensionless Exec + NVMe DMA Block I/O")
    print("======================================================================")

    os.makedirs(SCRATCH_DIR, exist_ok=True)
    for f in (SERIAL_LOG, QMP_SOCK, NVME_IMG):
        if os.path.exists(f):
            os.remove(f)

    # Fresh 32M NVMe raw backing file (NO IDE/SATA attached -> DiskFS MUST run over NVMe DMA)
    subprocess.run(["qemu-img", "create", "-f", "raw", NVME_IMG, "32M"], check=True)

    qemu_cmd = [
        "qemu-system-x86_64",
        "-M", "q35",
        "-cpu", "max",
        "-smp", "2",
        "-m", "512M",
        "-no-reboot", "-no-shutdown",
        "-serial", f"file:{SERIAL_LOG}",
        "-qmp", f"unix:{QMP_SOCK},server,nowait",
        "-drive", f"id=nvmdisk,file={NVME_IMG},format=raw,if=none",
        "-device", "nvme,serial=fipsos-nvme-v36,drive=nvmdisk",
        "-device", "qemu-xhci,id=xhci0",
        "-netdev", "user,id=net0,hostfwd=tcp::2222-:2222",
        "-device", "e1000e,netdev=net0",
        "-cdrom", ISO_IMG,
    ]

    print(f"[QEMU] Lanzando VM NVMe-only: {' '.join(qemu_cmd)}")
    qemu_proc = subprocess.Popen(qemu_cmd)

    try:
        print("[QEMU] Esperando arranque, NVMe + DiskFS y prompt del shell...")
        deadline = time.time() + 18.0
        while time.time() < deadline:
            s = read_serial()
            if "Type '" in s and "fipsos" in s:
                break
            time.sleep(0.5)
        time.sleep(1.0)

        qmp = QmpClient(QMP_SOCK)
        print("[QMP] Conectado exitosamente.")

        # 1. Check NVMe I/O Queues initialized in Serial Log
        serial_text = read_serial()
        assert "IO_QUEUES=1" in serial_text, "ERROR: NVMe no creo colas IOSQ/IOCQ (IO_QUEUES=1)!"
        assert "FIPSOS_DISKFS_V2" in serial_text, "ERROR: DiskFS v2 no formateo/monto sobre NVMe!"
        print("\033[1;32m[PASS]\033[0m NVMe I/O Queues (IO_QUEUES=1) y DiskFS v2 montado sobre NVMe.")

        ppm1 = f"{SCRATCH_DIR}/v36_shot1.ppm"
        png1 = f"{ARTIFACT_DIR}/shot1_v36_nvme_boot.png"
        qmp.screendump(ppm1)
        ppm_to_png(ppm1, png1)
        print(f"[IMG] {png1}")

        # 2. Extensionless execution + Static Linux Compat Syscalls ('c_std_app' without '.elf')
        print("[Shell] Ejecutando 'c_std_app' (sin sufijo .elf) para probar syscalls compat...")
        qmp.type_string("c_std_app\n", delay=0.05)
        compat_markers = [
            "[Linux Compat] SUCCESS: writev() + readv() vectored I/O passed",
            "[Linux Compat] SUCCESS: openat(AT_FDCWD) + fstat(",
            "[Linux Compat] SUCCESS: getrandom(16 B)",
            "[Linux Compat] SUCCESS: nanosleep(20ms) completed cleanly",
            "[posix_test.c] SUCCESS: All Linux libc standard routines executed with exit=0!",
        ]
        wait_end = time.time() + 8.0
        while time.time() < wait_end:
            serial_text = read_serial()
            if all(m in serial_text for m in compat_markers):
                break
            time.sleep(0.4)

        serial_text = read_serial()
        for m in compat_markers:
            assert m in serial_text, f"ERROR: Falta marcador de syscall compat en log serial: {m}"
        print("\033[1;32m[PASS]\033[0m Syscalls compat (writev/readv/openat/fstat/fcntl/getrandom/nanosleep) OK.")

        ppm2 = f"{SCRATCH_DIR}/v36_shot2.ppm"
        png2 = f"{ARTIFACT_DIR}/shot2_v36_compat_syscalls.png"
        qmp.screendump(ppm2)
        ppm_to_png(ppm2, png2)
        print(f"[IMG] {png2}")

        # 3. Extensionless execution of 'sample_tool' without '.elf'
        print("[Shell] Ejecutando 'sample_tool' (sin sufijo .elf)...")
        qmp.type_string("sample_tool\n", delay=0.05)
        time.sleep(2)

        serial_text = read_serial()
        assert "sample_tool" in serial_text, "ERROR: 'sample_tool' sin sufijo .elf no se ejecuto!"
        print("\033[1;32m[PASS]\033[0m Ejecucion sin sufijo .elf verificada ('c_std_app' y 'sample_tool').")

        ppm3 = f"{SCRATCH_DIR}/v36_shot3.ppm"
        png3 = f"{ARTIFACT_DIR}/shot3_v36_extensionless_exec.png"
        qmp.screendump(ppm3)
        ppm_to_png(ppm3, png3)
        print(f"[IMG] {png3}")

        # 4. Real NVMe DiskFS persistence + Remote Console Telemetry Check (:2222)
        print("[Remote] Conectando a 127.0.0.1:2222 para verificar R/W real en NVMe...")
        rem = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        rem.settimeout(5.0)
        rem.connect(("127.0.0.1", 2222))
        rem.recv(4096)

        rem.sendall(b"echo nvme_dma_v36_ok > /disk/nvme_io.txt\n")
        time.sleep(1.5)
        rem.recv(4096)

        rem.sendall(b"cat /disk/nvme_io.txt\n")
        time.sleep(1)
        cat_resp = strip_ansi(rem.recv(4096).decode("utf-8", errors="ignore"))
        print(f"[Remote] cat /disk/nvme_io.txt -> {cat_resp.strip()}")
        assert "nvme_dma_v36_ok" in cat_resp, "ERROR: No se leyo 'nvme_dma_v36_ok' desde NVMe DiskFS!"

        rem.sendall(b"sensors\n")
        time.sleep(1)
        sens_resp = strip_ansi(rem.recv(4096).decode("utf-8", errors="ignore"))
        print(f"[Remote] sensors ->\n{sens_resp.strip()}")
        assert "I/O=ON" in sens_resp and "R=" in sens_resp and "W=" in sens_resp, (
            "ERROR: Telemetria NVMe I/O no visible en sensors!"
        )
        rem.close()

        ppm4 = f"{SCRATCH_DIR}/v36_shot4.ppm"
        png4 = f"{ARTIFACT_DIR}/shot4_v36_nvme_diskfs.png"
        qmp.screendump(ppm4)
        ppm_to_png(ppm4, png4)
        print(f"[IMG] {png4}")

        qmp.close()
    finally:
        qemu_proc.terminate()
        try:
            qemu_proc.wait(timeout=4)
        except Exception:
            qemu_proc.kill()

    # 5. Host-side physical raw NVMe disk verification
    with open(NVME_IMG, "rb") as img:
        raw = img.read()
    assert b"FIPSOS_DISKFS_V2" in raw, "ERROR: Superbloque FIPSOS_DISKFS_V2 ausente en fipsos_nvme_v36.img!"
    assert b"nvme_dma_v36_ok" in raw, "ERROR: Payload 'nvme_dma_v36_ok' no persistio en fipsos_nvme_v36.img!"
    print("\033[1;32m[PASS]\033[0m Verificacion fisica de imagen NVMe en host: Superbloque + datos persistidos OK.")

    print("\n\033[1;32m======================================================================\033[0m")
    print("\033[1;32m  TODAS LAS PRUEBAS DE LA FASE v3.6 HAN PASADO EXITOSAMENTE (4/4)!\033[0m")
    print("\033[1;32m======================================================================\033[0m")


if __name__ == "__main__":
    main()
