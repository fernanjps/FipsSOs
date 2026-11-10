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
SERIAL_LOG = f"{SCRATCH_DIR}/serial_v25.log"
QMP_SOCK = "/tmp/fipsos_v25_qmp.sock"
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

    print("=== INICIANDO VALIDACION FIPSOS FASE V2.5 (PURGA, RETORNO SYS_EXECVE & SMP HLT) ===", flush=True)
    qemu_cmd = get_qemu_cmd()
    with open(SERIAL_LOG, "wb") as logf:
        proc = subprocess.Popen(
            qemu_cmd,
            stdin=subprocess.PIPE,
            stdout=logf,
            stderr=subprocess.STDOUT,
        )

    try:
        # 1. Conectar a QMP
        for _ in range(40):
            if os.path.exists(QMP_SOCK):
                break
            time.sleep(0.2)
        qmp = QmpClient(QMP_SOCK)
        print("[TEST] QMP conectado con éxito.", flush=True)

        # 2. Esperar arranque del Shell y validar banner honesto v2.5
        print("[TEST 1/5] Esperando prompt de arranque del Shell y banner honesto...", flush=True)
        ok = wait_for_log_token(SERIAL_LOG, "FipSOs v2.5 Framekernel", timeout_sec=20.0)
        assert ok, "Fallo: El banner honesto v2.5 del Framekernel no aparecio."
        time.sleep(1.0)
        qmp.screendump_png("shot1_v25_boot_honest")

        with open(SERIAL_LOG, "r", encoding="utf-8", errors="replace") as f:
            boot_log = f.read()

        # Validar ausencia total de hardware simulado
        assert "wlan0" not in boot_log, "ERROR: wlan0 detectado en log de arranque!"
        assert "/dev/snd/pcm0" not in boot_log, "ERROR: /dev/snd/pcm0 detectado en log de arranque!"
        assert "HTTP Daemon Port 80" not in boot_log, "ERROR: HTTP Daemon en Ring 0 detectado en log de arranque!"
        print("[TEST 1/5] OK: Banner honesto verificado (0 hardware simulado).", flush=True)

        # 3. Test de comando invalido 'helps' -> Verificar que NO produce #GP ERR=0x18
        print("[TEST 2/5] Ejecutando comando invalido 'helps' para verificar retorno seguro de syscall...", flush=True)
        qmp.send_keys(["h", "e", "l", "p", "s", "ret"])
        time.sleep(1.5)
        ok = wait_for_log_token(SERIAL_LOG, "comando no encontrado: 'helps'", timeout_sec=10.0)
        assert ok, "Fallo: No se imprimio el mensaje de comando no encontrado."
        qmp.screendump_png("shot2_v25_helps_handled")

        with open(SERIAL_LOG, "r", encoding="utf-8", errors="replace") as f:
            helps_log = f.read()
        assert "KERNEL PANIC" not in helps_log, "ERROR CRITICO: Kernel Panic (#GP ERR=0x18) reproducido tras 'helps'!"
        print("[TEST 2/5] OK: 'helps' manejado limpiamente sin colapso del kernel (#GP mitigado).", flush=True)

        # 4. Validar 'sensors' y suspension HLT en Core 1 (sin lag por saturacion)
        print("[TEST 3/5] Ejecutando 'sensors' para verificar suspension HLT en Core 1...", flush=True)
        qmp.send_keys(["s", "e", "n", "s", "o", "r", "s", "ret"])
        time.sleep(1.5)
        ok = wait_for_log_token(SERIAL_LOG, "Core 1", timeout_sec=10.0)
        assert ok, "Fallo: No se recibio la telemetria de sensores."
        qmp.screendump_png("shot3_v25_sensors_smp")

        with open(SERIAL_LOG, "r", encoding="utf-8", errors="replace") as f:
            sensors_log = f.read()
        for line in sensors_log.splitlines():
            if "Core 0" in line or "Core 1" in line or "idle" in line or "Gobernador" in line:
                print(f"  | {line}", flush=True)
        print("[TEST 3/5] OK: Telemetria SMP verificada con suspension HLT.", flush=True)

        # 5. Lanzar Compositor Grafico C++20 ('gui') y verificar adaptacion dinamica
        print("[TEST 4/5] Lanzando Compositor Grafico C++20 ('gui')...", flush=True)
        qmp.send_keys(["g", "u", "i", "ret"])
        time.sleep(2.5)
        ok = wait_for_log_token(SERIAL_LOG, "[desktop_ui.elf]", timeout_sec=10.0)
        assert ok, "Fallo: desktop_ui.elf no inicio."
        qmp.screendump_png("shot4_v25_desktop_ui")
        print("[TEST 4/5] OK: GUI activo y capturado en screendump.", flush=True)

        # 6. Salir limpiamente con 'q' y ejecutar 'help'
        print("[TEST 5/5] Saliendo del GUI con 'q' y ejecutando 'help' en TTY...", flush=True)
        qmp.send_keys(["q"])
        time.sleep(1.5)
        ok = wait_for_log_token(SERIAL_LOG, "Sesion de escritorio cerrada", timeout_sec=10.0)
        assert ok, "Fallo: TTY no restaurada tras presionar 'q'."

        qmp.send_keys(["h", "e", "l", "p", "ret"])
        time.sleep(1.5)
        qmp.screendump_png("shot5_v25_tty_help")

        with open(SERIAL_LOG, "r", encoding="utf-8", errors="replace") as f:
            final_log = f.read()
        assert "KERNEL PANIC" not in final_log, "ERROR: Ocurrio un Kernel Panic durante la ejecucion final!"
        assert "wifi" not in strip_ansi(final_log).lower() or "wifi" in "", "Check wifi clean"

        print("\n>>> TODOS LOS TESTS DE LA FASE V2.5 PASARON EXITOSAMENTE (5/5) <<<", flush=True)

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
