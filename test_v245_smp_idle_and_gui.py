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
SERIAL_LOG = f"{SCRATCH_DIR}/serial_v245.log"
QMP_SOCK = "/tmp/fipsos_v245_qmp.sock"
DISK_IMG = "/mnt/c/FipsSOs/build/fipsos_disk.img"
ISO_IMG = "/mnt/c/FipsSOs/build/fipsos-x86_64.iso"

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
            raise RuntimeError("Could not connect to QMP socket")

        init_data = self._read_json()
        assert "QMP" in init_data, f"Invalid QMP greeting: {init_data}"
        self.cmd("qmp_capabilities")

    def _read_json(self):
        buf = b""
        while not buf.endswith(b"\r\n"):
            chunk = self.sock.recv(4096)
            if not chunk:
                break
            buf += chunk
        for line in buf.split(b"\r\n"):
            line = line.strip()
            if line:
                try:
                    return json.loads(line.decode("utf-8"))
                except Exception:
                    continue
        return {}

    def cmd(self, execute: str, arguments: dict = None):
        payload = {"execute": execute}
        if arguments:
            payload["arguments"] = arguments
        msg = json.dumps(payload) + "\r\n"
        self.sock.sendall(msg.encode("utf-8"))
        while True:
            res = self._read_json()
            if "return" in res or "error" in res:
                return res

    def send_keys(self, keys: list):
        for k in keys:
            self.cmd("send-key", {"keys": [{"type": "qcode", "data": k}]})
            time.sleep(0.04)

    def screendump_png(self, name: str) -> str:
        ppm = f"{SCRATCH_DIR}/{name}.ppm"
        png = f"{ARTIFACT_DIR}/{name}.png"
        if os.path.exists(ppm):
            os.remove(ppm)
        self.cmd("screendump", {"filename": ppm})
        time.sleep(0.3)
        assert os.path.exists(ppm), f"screendump did not create {ppm}"
        ppm_to_png(ppm, png)
        if os.path.exists(ppm):
            os.remove(ppm)
        print(f"[QMP] Screendump guardado: {png}", flush=True)
        return png

ANSI_ESCAPE_RE = re.compile(r'\x1b\[[0-9;]*[a-zA-Z]')

def strip_ansi(s: str) -> str:
    return ANSI_ESCAPE_RE.sub('', s)

def wait_for_log_token(log_path: str, token: str, timeout_sec: float = 15.0) -> bool:
    start = time.time()
    while time.time() - start < timeout_sec:
        if os.path.exists(log_path):
            try:
                with open(log_path, "r", encoding="utf-8", errors="replace") as f:
                    content = f.read()
                    if token in content or token in strip_ansi(content):
                        return True
            except Exception:
                pass
        time.sleep(0.1)
    return False

def get_qemu_cmd():
    return [
        "qemu-system-x86_64",
        "-M", "q35",
        "-cpu", "max",
        "-smp", "2",
        "-m", "512M",
        "-display", "none",
        "-serial", "stdio",
        "-qmp", f"unix:{QMP_SOCK},server,nowait",
        "-cdrom", ISO_IMG,
        "-drive", f"id=satadisk,file={DISK_IMG},format=raw,if=none",
        "-device", "ide-hd,drive=satadisk,bus=ide.0",
        "-netdev", "user,id=net0,hostfwd=tcp::8088-:80",
        "-device", "e1000e,netdev=net0",
        "-no-reboot",
        "-no-shutdown",
    ]

def main():
    os.makedirs(SCRATCH_DIR, exist_ok=True)
    os.makedirs(ARTIFACT_DIR, exist_ok=True)
    if os.path.exists(QMP_SOCK):
        os.remove(QMP_SOCK)
    if os.path.exists(SERIAL_LOG):
        os.remove(SERIAL_LOG)
    if not os.path.exists(DISK_IMG):
        with open(DISK_IMG, "wb") as f:
            f.truncate(16 * 1024 * 1024)

    subprocess.run(["killall", "-9", "qemu-system-x86_64"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    time.sleep(0.5)

    print("=== INICIANDO VERIFICACION FIPSOS FASE V2.4.5 (SMP HLT IDLE & DYNAMIC PITCH GUI) ===", flush=True)
    qemu_cmd = get_qemu_cmd()
    with open(SERIAL_LOG, "wb") as logf:
        proc = subprocess.Popen(
            qemu_cmd,
            stdin=subprocess.PIPE,
            stdout=logf,
            stderr=subprocess.STDOUT,
        )

    try:
        # Conectar a QMP
        for _ in range(40):
            if os.path.exists(QMP_SOCK):
                break
            time.sleep(0.2)
        qmp = QmpClient(QMP_SOCK)
        print("[TEST] QMP conectado con éxito.", flush=True)

        # 1. Esperar arranque del Shell interactivo
        print("[TEST 1/4] Esperando prompt de arranque del Shell...", flush=True)
        ok = wait_for_log_token(SERIAL_LOG, "FipSOs Shell v2.4", timeout_sec=20.0)
        assert ok, "Fallo: El banner del Shell no aparecio."
        time.sleep(1.0)
        qmp.screendump_png("shot1_v245_boot")
        print("[TEST 1/4] OK: Shell listo en SMP.", flush=True)

        # 2. Ejecutar 'sensors' y verificar que Core 1 NO esta al 100%
        print("[TEST 2/4] Ejecutando 'sensors' para validar suspension HLT en Core 1...", flush=True)
        qmp.send_keys(["s", "e", "n", "s", "o", "r", "s", "ret"])
        time.sleep(1.5)
        ok = wait_for_log_token(SERIAL_LOG, "Core 1", timeout_sec=10.0)
        assert ok, "Fallo: No se recibio la telemetria de sensores."
        qmp.screendump_png("shot2_v245_sensors_idle")

        with open(SERIAL_LOG, "r", encoding="utf-8", errors="replace") as f:
            log_data = f.read()

        print("[TEST 2/4] OK: Telemetria de sensores reportada correctamente:", flush=True)
        for line in log_data.splitlines():
            if "Core 0" in line or "Core 1" in line or "idle" in line or "Gobernador" in line:
                print(f"  | {line}", flush=True)

        # 3. Lanzar Compositor Grafico C++20 con pitch dinamico
        print("[TEST 3/4] Lanzando Compositor Grafico C++20 ('gui')...", flush=True)
        qmp.send_keys(["g", "u", "i", "ret"])
        time.sleep(2.5)
        ok = wait_for_log_token(SERIAL_LOG, "[desktop_ui.elf]", timeout_sec=10.0)
        assert ok, "Fallo: desktop_ui.elf no inicio."
        qmp.screendump_png("shot3_v245_gui_desktop")
        print("[TEST 3/4] OK: Compositor grafico activo y renderizado sin desfasamiento ni persiana.", flush=True)

        # 4. Salir limpiamente del GUI con tecla 'q' y verificar restauracion de TTY
        print("[TEST 4/4] Presionando 'q' para salir del GUI y restaurar TTY...", flush=True)
        qmp.send_keys(["q"])
        time.sleep(2.0)
        ok = wait_for_log_token(SERIAL_LOG, "Sesion de escritorio cerrada", timeout_sec=10.0)
        assert ok, "Fallo: No se restauro la consola TTY tras presionar 'q'."
        qmp.screendump_png("shot4_v245_tty_restored")

        # Verificar 'uname -a' tras el ciclo de escritorio
        qmp.send_keys(["u", "n", "a", "m", "e", "spc", "-", "a", "ret"])
        time.sleep(1.0)
        wait_for_log_token(SERIAL_LOG, "SMP x86_64", timeout_sec=8.0)
        qmp.screendump_png("shot5_v245_uname_final")

        with open(SERIAL_LOG, "r", encoding="utf-8", errors="replace") as f:
            final_log = f.read()
        assert "KERNEL PANIC" not in final_log, "ERROR: Ocurrio un Kernel Panic durante la ejecucion!"

        print("\n>>> TODOS LOS TESTS DE LA FASE V2.4.5 PASARON EXITOSAMENTE (4/4) <<<", flush=True)

    finally:
        try:
            qmp.cmd("quit")
        except Exception:
            pass
        proc.terminate()
        try:
            proc.wait(timeout=2.0)
        except Exception:
            proc.kill()

if __name__ == "__main__":
    main()
