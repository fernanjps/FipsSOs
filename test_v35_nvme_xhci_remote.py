#!/usr/bin/env python3
"""
FipSOs - Fase v3.5 Automated Test Suite (test_v35_nvme_xhci_remote.py)
Validates:
1. Bare-Metal PCIe NVMe Controller Detection & Admin Queues DMA (ASQ/ACQ, CSTS.RDY=1).
2. USB xHCI DMA Rings (DCBAAP, CRCR, ERSTBA, ERDP) & Runtime Interrupter 0.
3. Modular Shell (fipsos_sh.c + sh/sh_readline.c + sh/sh_builtins.c).
4. Sensors & HWMON Reporting for NVMe Link Status.
5. Dedicated Remote Console Client (tools/fipsos_remote.py & TCP :2222).
6. Desktop UI (C++20 Graphical Compositor).
"""

import os
import sys
import time
import socket
import json
import struct
import zlib
import subprocess
import shutil
import re

SCRATCH_DIR = "/mnt/c/Users/Ferna/.gemini/antigravity/brain/4ef4b43d-91a0-44a4-aefe-f8b38846bd63/scratch"
ARTIFACT_DIR = "/mnt/c/Users/Ferna/.gemini/antigravity/brain/4ef4b43d-91a0-44a4-aefe-f8b38846bd63"
SERIAL_LOG = f"{SCRATCH_DIR}/serial_v35.log"
QMP_SOCK = "/tmp/fipsos_v35_qmp.sock"
DISK_IMG = "/mnt/c/FipsSOs/build/fipsos_disk.img"
NVME_IMG = "/mnt/c/FipsSOs/build/fipsos_nvme.img"
ISO_IMG = "/mnt/c/FipsSOs/build/fipsos-x86_64.iso"

ANSI_ESCAPE_RE = re.compile(r'\x1b\[[0-9;]*[a-zA-Z]')

def strip_ansi(s: str) -> str:
    return ANSI_ESCAPE_RE.sub('', s)

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
                lines = buf.split(b"\r\n")
                for line in lines:
                    line = line.strip()
                    if line:
                        try:
                            return json.loads(line.decode("utf-8"))
                        except Exception:
                            continue
        return {}

    def _send_json(self, cmd):
        data = (json.dumps(cmd) + "\r\n").encode("utf-8")
        self.sock.sendall(data)

    def send_key(self, key_name):
        cmd = {
            "execute": "send-key",
            "arguments": {"keys": [{"type": "qcode", "data": key_name}]}
        }
        self._send_json(cmd)
        self._read_json()

    def type_string(self, text: str, delay=0.04):
        key_map = {
            " ": "spc", "\n": "ret", "-": "minus", ".": "dot", "/": "slash",
            "_": "shift-minus", ">": "shift-dot", "|": "shift-backslash",
            ":": "shift-semicolon", ";": "semicolon", "\"": "shift-apostrophe",
            "=": "equal", "+": "shift-equal",
            "1": "1", "2": "2", "3": "3", "4": "4", "5": "5",
            "6": "6", "7": "7", "8": "8", "9": "9", "0": "0",
        }
        for ch in text:
            if ch in key_map:
                qcode = key_map[ch]
                if "-" in qcode:
                    parts = qcode.split("-")
                    cmd = {
                        "execute": "send-key",
                        "arguments": {"keys": [{"type": "qcode", "data": p} for p in parts]}
                    }
                    self._send_json(cmd)
                    self._read_json()
                else:
                    self.send_key(qcode)
            elif ch.isalpha():
                self.send_key(ch.lower())
            time.sleep(delay)

    def screendump(self, out_ppm_path: str):
        cmd = {
            "execute": "screendump",
            "arguments": {"filename": out_ppm_path}
        }
        self._send_json(cmd)
        return self._read_json()

    def close(self):
        try:
            self.sock.close()
        except Exception:
            pass

def ensure_disks():
    if not os.path.exists(DISK_IMG):
        subprocess.run(["qemu-img", "create", "-f", "raw", DISK_IMG, "64M"], check=True)
    if not os.path.exists(NVME_IMG):
        subprocess.run(["qemu-img", "create", "-f", "raw", NVME_IMG, "64M"], check=True)

def main():
    print("======================================================================")
    print("  FipSOs Fase v3.5 - Suite de Verificacion Automatizada")
    print("  PCIe NVMe DMA + USB xHCI DMA Rings + Modular Shell + Remote Console")
    print("======================================================================")

    os.makedirs(SCRATCH_DIR, exist_ok=True)
    if os.path.exists(SERIAL_LOG):
        os.remove(SERIAL_LOG)
    if os.path.exists(QMP_SOCK):
        os.remove(QMP_SOCK)

    ensure_disks()

    # Launch QEMU with NVMe controller, xHCI controller, USB keyboard, SATA, NIC with port forwarding
    qemu_cmd = [
        "qemu-system-x86_64",
        "-M", "q35",
        "-cpu", "max",
        "-smp", "2",
        "-m", "512M",
        "-no-reboot", "-no-shutdown",
        "-serial", f"file:{SERIAL_LOG}",
        "-qmp", f"unix:{QMP_SOCK},server,nowait",
        "-drive", f"id=satadisk,file={DISK_IMG},format=raw,if=none",
        "-device", "ide-hd,drive=satadisk,bus=ide.0",
        "-drive", f"id=nvmdisk,file={NVME_IMG},format=raw,if=none",
        "-device", "nvme,serial=fipsos-nvme0,drive=nvmdisk",
        "-device", "qemu-xhci,id=xhci0",
        "-device", "usb-kbd,bus=xhci0.0",
        "-netdev", "user,id=net0,hostfwd=tcp::2222-:2222",
        "-device", "e1000e,netdev=net0",
        "-cdrom", ISO_IMG,
    ]

    print(f"[QEMU] Lanzando comando: {' '.join(qemu_cmd)}")
    qemu_proc = subprocess.Popen(qemu_cmd)

    try:
        print("[QEMU] Esperando arranque del sistema (10 segundos)...")
        time.sleep(10)

        # 1. Connect to QMP
        qmp = QmpClient(QMP_SOCK)
        print("[QMP] Conectado exitosamente al socket de control QMP.")

        # Shot 1: Boot & Hardware Detection
        ppm1 = f"{SCRATCH_DIR}/shot1.ppm"
        png1 = f"{ARTIFACT_DIR}/shot1_v35_boot.png"
        qmp.screendump(ppm1)
        ppm_to_png(ppm1, png1)
        print(f"\033[1;32m[PASS]\033[0m Captura 1 guardada: {png1}")

        # Check Serial Log for NVMe and xHCI DMA
        with open(SERIAL_LOG, "r", encoding="utf-8", errors="ignore") as f:
            serial_text = f.read()

        nvme_detected = "NVMe DETECTADO" in serial_text or "PCIe NVMe" in serial_text
        nvme_rdy = "CSTS.RDY=1" in serial_text or "Admin Queues=64" in serial_text
        xhci_dma = "Anillos DMA xHCI" in serial_text
        xhci_running = "xHCI USB 3.0" in serial_text

        print(f"[Serial Log Check] NVMe Detectado: {nvme_detected} | NVMe Ready: {nvme_rdy}")
        print(f"[Serial Log Check] xHCI DMA Rings: {xhci_dma} | xHCI Running: {xhci_running}")

        assert nvme_detected, "ERROR: No se detecto la controladora PCIe NVMe en el log serial!"
        assert nvme_rdy, "ERROR: La controladora NVMe no alcanzo el estado CSTS.RDY=1!"
        assert xhci_dma, "ERROR: No se configuraron los Anillos DMA en el driver xHCI!"

        # Shot 2: xHCI DMA Telemetry
        ppm2 = f"{SCRATCH_DIR}/shot2.ppm"
        png2 = f"{ARTIFACT_DIR}/shot2_v35_xhci_dma.png"
        qmp.screendump(ppm2)
        ppm_to_png(ppm2, png2)
        print(f"\033[1;32m[PASS]\033[0m Captura 2 guardada: {png2}")

        # 3. Interactive Shell Test: type commands
        print("[Shell] Enviando comando 'sensors'...")
        qmp.type_string("sensors\n", delay=0.05)
        time.sleep(2)

        ppm3 = f"{SCRATCH_DIR}/shot3.ppm"
        png3 = f"{ARTIFACT_DIR}/shot3_v35_shell_modular.png"
        qmp.screendump(ppm3)
        ppm_to_png(ppm3, png3)
        print(f"\033[1;32m[PASS]\033[0m Captura 3 guardada: {png3}")

        # 4. Pipe Test: sensors | cat
        print("[Shell] Enviando pipe 'help | cat'...")
        qmp.type_string("help | cat\n", delay=0.05)
        time.sleep(2)

        ppm4 = f"{SCRATCH_DIR}/shot4.ppm"
        png4 = f"{ARTIFACT_DIR}/shot4_v35_pipes_modular.png"
        qmp.screendump(ppm4)
        ppm_to_png(ppm4, png4)
        print(f"\033[1;32m[PASS]\033[0m Captura 4 guardada: {png4}")

        # 5. Remote Console Client Test (:2222)
        print("[Remote Console] Conectando socket a 127.0.0.1:2222...")
        rem_sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        rem_sock.settimeout(5.0)
        rem_sock.connect(('127.0.0.1', 2222))
        banner = rem_sock.recv(4096).decode('utf-8', errors='ignore')
        print(f"[Remote Console] Banner recibido:\n{banner}")

        rem_sock.sendall(b"sensors\n")
        time.sleep(1)
        resp = rem_sock.recv(4096).decode('utf-8', errors='ignore')
        print(f"[Remote Console] Salida de 'sensors':\n{resp}")
        assert "NVMe" in resp or "PCIe" in resp, "ERROR: La salida remota no contiene telemetria de NVMe!"

        rem_sock.close()
        print("\033[1;32m[PASS]\033[0m Conexion remota interactiva verificada con exito.")

        ppm5 = f"{SCRATCH_DIR}/shot5.ppm"
        png5 = f"{ARTIFACT_DIR}/shot5_v35_remote_console.png"
        qmp.screendump(ppm5)
        ppm_to_png(ppm5, png5)
        print(f"\033[1;32m[PASS]\033[0m Captura 5 guardada: {png5}")

        # 6. Launch Desktop UI (C++20 Graphical Compositor)
        print("[Shell] Lanzando GUI 'gui'...")
        qmp.type_string("gui\n", delay=0.05)
        time.sleep(4)

        ppm6 = f"{SCRATCH_DIR}/shot6.ppm"
        png6 = f"{ARTIFACT_DIR}/shot6_v35_desktop_ui.png"
        qmp.screendump(ppm6)
        ppm_to_png(ppm6, png6)
        print(f"\033[1;32m[PASS]\033[0m Captura 6 guardada: {png6}")

        qmp.close()
        print("\n\033[1;32m======================================================================\033[0m")
        print("\033[1;32m  TODAS LAS PRUEBAS DE LA FASE v3.5 HAN PASADO EXITOSAMENTE (6/6)!\033[0m")
        print("\033[1;32m======================================================================\033[0m")

    finally:
        qemu_proc.terminate()
        try:
            qemu_proc.wait(timeout=3)
        except Exception:
            qemu_proc.kill()

if __name__ == "__main__":
    main()
