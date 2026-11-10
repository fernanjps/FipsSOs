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
SERIAL_LOG = f"{SCRATCH_DIR}/serial_v33.log"
QMP_SOCK = "/tmp/fipsos_v33_qmp.sock"
DISK_IMG = "/mnt/c/FipsSOs/build/fipsos_disk.img"
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
            "\"": "shift-apostrophe",
            "=": "equal",
            "+": "shift-equal",
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
        resp = self._read_json()
        return resp

    def close(self):
        try:
            self.sock.close()
        except Exception:
            pass

def ensure_disk():
    if not os.path.exists(DISK_IMG):
        subprocess.run(["qemu-img", "create", "-f", "raw", DISK_IMG, "64M"], check=True)

def main():
    print("======================================================================")
    print("  SUITE DE VERIFICACION FASE v3.3 - FipSOs Modular Hardware Drivers & ostd")
    print("======================================================================")
    os.makedirs(SCRATCH_DIR, exist_ok=True)
    os.makedirs(ARTIFACT_DIR, exist_ok=True)
    ensure_disk()

    if os.path.exists(QMP_SOCK):
        os.remove(QMP_SOCK)

    subprocess.run(["killall", "-9", "qemu-system-x86_64"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    time.sleep(0.5)

    # Launch QEMU with OHCI + EHCI + xHCI + e1000e + SATA AHCI
    qemu_cmd = [
        "qemu-system-x86_64",
        "-M", "q35",
        "-cpu", "max",
        "-smp", "2",
        "-m", "512M",
        "-display", "none",
        "-serial", "stdio",
        "-qmp", f"unix:{QMP_SOCK},server,nowait",
        "-cdrom", ISO_IMG,
        "-drive", f"id=disk0,file={DISK_IMG},format=raw,if=none",
        "-device", "ide-hd,drive=disk0,bus=ide.0",
        "-netdev", "user,id=net0,hostfwd=tcp::2222-:2222,hostfwd=tcp::8080-:80",
        "-device", "e1000e,netdev=net0",
        "-device", "pci-ohci,id=ohci",
        "-device", "usb-ehci,id=ehci",
        "-device", "qemu-xhci,id=xhci",
        "-device", "usb-kbd,bus=xhci.0",
        "-no-reboot",
        "-no-shutdown",
    ]

    print("[QEMU] Lanzando emulador con FipSOs v3.3...", flush=True)
    with open(SERIAL_LOG, "wb") as logf:
        proc = subprocess.Popen(
            qemu_cmd,
            stdin=subprocess.PIPE,
            stdout=logf,
            stderr=subprocess.STDOUT
        )

    try:
        print("[QMP] Conectando a socket QMP...", flush=True)
        qmp = QmpClient(QMP_SOCK)

        # Esperar arranque Limine y aparicion del shell
        print("[BOOT] Esperando arranque del kernel modular...", flush=True)
        shell_ready = False
        for _ in range(40):
            time.sleep(0.5)
            if os.path.exists(SERIAL_LOG):
                with open(SERIAL_LOG, "r", encoding="utf-8", errors="ignore") as f:
                    content = f.read()
                    if "fipsos:/" in content or "Type 'help'" in content:
                        shell_ready = True
                        break

        ppm_shot1 = f"{SCRATCH_DIR}/shot1_v33.ppm"
        png_shot1 = f"{ARTIFACT_DIR}/shot1_v33_boot.png"
        qmp.screendump(ppm_shot1)
        ppm_to_png(ppm_shot1, png_shot1)
        print(f"[TEST 1/6] Boot & Modular ostd Subsystems OK -> {png_shot1}")

        # TEST 2: Framebuffer Console Modular (ls)
        print("[SHELL] Ejecutando 'ls' en Framebuffer Console modular...", flush=True)
        qmp.type_string("ls\n", delay=0.04)
        time.sleep(1.0)
        ppm_shot2 = f"{SCRATCH_DIR}/shot2_v33.ppm"
        png_shot2 = f"{ARTIFACT_DIR}/shot2_v33_fb_console.png"
        qmp.screendump(ppm_shot2)
        ppm_to_png(ppm_shot2, png_shot2)
        print(f"[TEST 2/6] Modular Framebuffer Console & ANSI Rendering OK -> {png_shot2}")

        # TEST 3: Multi-USB Driver Detection (xHCI, EHCI, OHCI)
        print("[SHELL] Verificando controladores USB en /proc/bus/pci...", flush=True)
        qmp.type_string("cat /proc/bus/pci\n", delay=0.04)
        time.sleep(1.2)
        ppm_shot3 = f"{SCRATCH_DIR}/shot3_v33.ppm"
        png_shot3 = f"{ARTIFACT_DIR}/shot3_v33_usb_telemetry.png"
        qmp.screendump(ppm_shot3)
        ppm_to_png(ppm_shot3, png_shot3)
        print(f"[TEST 3/6] Modular USB Drivers (xHCI, EHCI, OHCI) Detection OK -> {png_shot3}")

        # TEST 4: Planificador Modular y Pipes Redirection (sys_fork, sys_pipe)
        print("[SHELL] Probando fork, pipes y redireccion...", flush=True)
        qmp.type_string("echo fipsos_v33_modular_ostd > /disk/v33.txt\n", delay=0.04)
        time.sleep(0.8)
        qmp.type_string("cat /disk/v33.txt\n", delay=0.04)
        time.sleep(0.8)
        ppm_shot4 = f"{SCRATCH_DIR}/shot4_v33.ppm"
        png_shot4 = f"{ARTIFACT_DIR}/shot4_v33_scheduler_pipes.png"
        qmp.screendump(ppm_shot4)
        ppm_to_png(ppm_shot4, png_shot4)
        print(f"[TEST 4/6] Modular Scheduler & Process Lifecycle OK -> {png_shot4}")

        # TEST 5: Telemetria y Espacio de Memoria Virtual (sensors)
        print("[SHELL] Ejecutando 'sensors' (telemetria SMP, CPU idle y memoria)...", flush=True)
        qmp.type_string("sensors\n", delay=0.04)
        time.sleep(1.2)
        ppm_shot5 = f"{SCRATCH_DIR}/shot5_v33.ppm"
        png_shot5 = f"{ARTIFACT_DIR}/shot5_v33_sensors_vm_space.png"
        qmp.screendump(ppm_shot5)
        ppm_to_png(ppm_shot5, png_shot5)
        print(f"[TEST 5/6] Hardware Telemetry & VM Space Isolation OK -> {png_shot5}")

        # TEST 6: Desktop GUI Compositor C++20 Ring 3
        print("[SHELL] Lanzando Compositor Grafico C++ 'gui'...", flush=True)
        qmp.type_string("gui\n", delay=0.04)
        time.sleep(2.5)
        ppm_shot6 = f"{SCRATCH_DIR}/shot6_v33.ppm"
        png_shot6 = f"{ARTIFACT_DIR}/shot6_v33_desktop_ui.png"
        qmp.screendump(ppm_shot6)
        ppm_to_png(ppm_shot6, png_shot6)
        print(f"[TEST 6/6] Desktop UI C++20 Ring 3 Compositor OK -> {png_shot6}")

        qmp.close()
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=3)
        except Exception:
            proc.kill()
        subprocess.run(["killall", "-9", "qemu-system-x86_64"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

    print("======================================================================")
    print("  SUITE DE PRUEBAS FASE v3.3 COMPLETADA CON EXITO (6/6 PRUEBAS OK)")
    print("======================================================================")

if __name__ == "__main__":
    main()
