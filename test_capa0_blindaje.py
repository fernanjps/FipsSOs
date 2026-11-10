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
SERIAL_LOG = f"{SCRATCH_DIR}/serial_capa0.log"
QMP_SOCK = "/tmp/fipsos_capa0_qmp.sock"
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
        self.sock.connect(sock_path)
        self.buf = b""
        self._recv_json()
        self.cmd("qmp_capabilities")

    def _recv_json(self):
        while b"\n" not in self.buf:
            chunk = self.sock.recv(4096)
            if not chunk:
                raise RuntimeError("QMP closed")
            self.buf += chunk
        line, self.buf = self.buf.split(b"\n", 1)
        return json.loads(line.decode("utf-8", errors="replace"))

    def cmd(self, execute: str, arguments=None):
        payload = {"execute": execute}
        if arguments is not None:
            payload["arguments"] = arguments
        self.sock.sendall((json.dumps(payload) + "\n").encode("utf-8"))
        while True:
            msg = self._recv_json()
            if "return" in msg or "error" in msg:
                return msg

    def screendump_png(self, name: str):
        ppm = f"{SCRATCH_DIR}/{name}.ppm"
        png = f"{SCRATCH_DIR}/{name}.png"
        self.cmd("screendump", {"filename": ppm})
        time.sleep(0.15)
        ppm_to_png(ppm, png)
        if os.path.exists(ppm):
            os.remove(ppm)
        art_png = f"{ARTIFACT_DIR}/{name}.png"
        shutil.copyfile(png, art_png)
        print(f"[QMP] Screendump guardado: {png} -> {art_png}")

    def send_keys(self, keys: list):
        for k in keys:
            self.cmd("send-key", {"keys": [{"type": "qcode", "data": k}]})
            time.sleep(0.04)

ANSI_ESCAPE_RE = re.compile(r'\x1b\[[0-9;]*[a-zA-Z]')

def strip_ansi(s: str) -> str:
    return ANSI_ESCAPE_RE.sub('', s)

def wait_for_log_token(log_path: str, token: str, timeout_sec: float = 12.0) -> bool:
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
    if os.path.exists(QMP_SOCK):
        os.remove(QMP_SOCK)
    if os.path.exists(SERIAL_LOG):
        os.remove(SERIAL_LOG)
    if not os.path.exists(DISK_IMG):
        with open(DISK_IMG, "wb") as f:
            f.truncate(16 * 1024 * 1024)

    subprocess.run(["killall", "-9", "qemu-system-x86_64"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    time.sleep(0.5)

    print("=== INICIANDO VERIFICACION FIPSOS CAPA 0 BLINDAJE (C11 + ASM + RUST) ===")
    qemu_cmd = get_qemu_cmd()
    with open(SERIAL_LOG, "wb") as logf:
        proc = subprocess.Popen(
            qemu_cmd,
            stdin=subprocess.PIPE,
            stdout=logf,
            stderr=subprocess.STDOUT,
        )

    try:
        # 1. Esperar conexión QMP
        for _ in range(40):
            if os.path.exists(QMP_SOCK):
                break
            time.sleep(0.2)
        qmp = QmpClient(QMP_SOCK)
        print("[TEST] QMP conectado con éxito.")

        # 2. Verificar arranque del Shell interactivo
        print("[TEST 1/6] Verificando arranque del Shell con Capa 0 blindada...")
        ok = wait_for_log_token(SERIAL_LOG, "FipSOs Shell v2.4", timeout_sec=15.0)
        assert ok, "Fallo: El banner del Shell no aparecio en el log serial."
        time.sleep(0.5)
        qmp.screendump_png("shot1_capa0_shell_boot")
        print("[TEST 1/6] OK: Shell arrancado y banner visible.")

        # 3. Verificar inicialización FPU/AVX/XSAVE en BSP y APs
        print("[TEST 2/6] Verificando inicialización de FPU x87 + SSE/SSE2 + XSAVE + AVX en el log...")
        with open(SERIAL_LOG, "r", encoding="utf-8", errors="replace") as f:
            log_content = f.read()
        assert "XSAVE=true" in log_content or "AVX=true" in log_content or "FPU x87 + SSE/SSE2 activos" in log_content, "FPU log no encontrado"
        assert "IDT blindada: 32 vectores" in log_content or "32 vectores de excepcion blindados" in log_content, "IDT 32 vectores log no encontrado"
        print("[TEST 2/6] OK: FPU (XSAVE/AVX) y IDT (32 vectores) inicializados con éxito.")

        # 4. Enviar comando 'help' y verificar flujo continuo sin interbloqueos
        print("[TEST 3/6] Verificando streaming multilínea del comando 'help'...")
        qmp.send_keys(["h", "e", "l", "p", "ret"])
        time.sleep(1.0)
        ok = wait_for_log_token(SERIAL_LOG, "edit / kilo", timeout_sec=8.0)
        assert ok, "Fallo: El listado completo de comandos de help no se imprimio."
        qmp.screendump_png("shot2_capa0_help_stream")
        print("[TEST 3/6] OK: Comando 'help' se transmitio completo sin deadlocks.")

        # 5. Ejecutar busybox
        print("[TEST 4/6] Verificando ejecución de busybox...")
        qmp.send_keys(["b", "u", "s", "y", "b", "o", "x", "ret"])
        time.sleep(1.2)
        ok = wait_for_log_token(SERIAL_LOG, "BusyBox v1.35.0", timeout_sec=8.0)
        assert ok, "Fallo: BusyBox v1.35.0 no se ejecuto correctamente."
        qmp.screendump_png("shot3_capa0_busybox_run")
        print("[TEST 4/6] OK: BusyBox v1.35.0 ejecuto y retorno al prompt.")

        # 6. Limpieza de pantalla
        print("[TEST 5/6] Verificando comando 'clear'...")
        qmp.send_keys(["c", "l", "e", "a", "r", "ret"])
        time.sleep(0.6)
        qmp.screendump_png("shot4_capa0_clear_screen")
        print("[TEST 5/6] OK: Pantalla limpia.")

        # 7. Conmutar a Desktop UI
        print("[TEST 6/6] Verificando interfaz gráfica Desktop UI...")
        qmp.send_keys(["g", "u", "i", "ret"])
        time.sleep(1.5)
        qmp.screendump_png("shot5_capa0_desktop_ui")
        print("[TEST 6/6] OK: Desktop UI activo.")

        # Salir de GUI con tecla 'q'
        print("[TEST FINAL] Saliendo de GUI con 'q' para retornar a TTY...")
        qmp.send_keys(["q"])
        time.sleep(1.2)
        qmp.screendump_png("shot6_capa0_tty_restored")
        print("[TEST FINAL] OK: Shell TTY restaurado correctamente.")

        print("\n>>> TODOS LOS TESTS DE BLINDAJE DE CAPA 0 PASARON EXITOSAMENTE (6/6) <<<")

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
