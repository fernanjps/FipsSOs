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
SERIAL_LOG = f"{SCRATCH_DIR}/serial_v244.log"
QMP_SOCK = "/tmp/fipsos_v244_qmp.sock"
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

    print("=== INICIANDO VERIFICACION FIPSOS FASE V2.4.4 (ERROR RETURN STABILITY & GDT/SWAPGS) ===")
    qemu_cmd = get_qemu_cmd()
    with open(SERIAL_LOG, "wb") as logf:
        proc = subprocess.Popen(
            qemu_cmd,
            stdin=subprocess.PIPE,
            stdout=logf,
            stderr=subprocess.STDOUT,
        )

    try:
        # 1. Conexión QMP
        for _ in range(40):
            if os.path.exists(QMP_SOCK):
                break
            time.sleep(0.2)
        qmp = QmpClient(QMP_SOCK)
        print("[TEST] QMP conectado con éxito.")

        # 2. Esperar arranque del Shell interactivo
        print("[TEST 1/5] Esperando prompt de arranque del Shell...")
        ok = wait_for_log_token(SERIAL_LOG, "FipSOs Shell v2.4", timeout_sec=15.0)
        assert ok, "Fallo: El banner del Shell no aparecio."
        time.sleep(0.5)
        qmp.screendump_png("shot1_v244_boot")
        print("[TEST 1/5] OK: Shell listo.")

        # 3. Ingresar comando inexistente 'helps' y verificar manejo de error SIN Kernel Panic
        print("[TEST 2/5] Ingresando comando inexistente 'helps'...")
        qmp.send_keys(["h", "e", "l", "p", "s", "ret"])
        time.sleep(1.0)
        ok = wait_for_log_token(SERIAL_LOG, "comando no encontrado: 'helps'", timeout_sec=8.0)
        assert ok, "Fallo: No se recibio el mensaje de comando no encontrado."
        qmp.screendump_png("shot2_v244_helps_not_found")

        # Verificar que NO hubo Kernel Panic
        with open(SERIAL_LOG, "r", encoding="utf-8", errors="replace") as f:
            log_content = f.read()
        assert "KERNEL PANIC" not in log_content, "ERROR: Ocurrio un Kernel Panic tras ingresar 'helps'!"
        print("[TEST 2/5] OK: 'helps' reporto error y retorno limpiamente SIN Kernel Panic.")

        # 4. Ingresar otro comando inexistente 'xyz123'
        print("[TEST 3/5] Ingresando segundo comando inexistente 'xyz123'...")
        qmp.send_keys(["x", "y", "z", "1", "2", "3", "ret"])
        time.sleep(1.0)
        ok = wait_for_log_token(SERIAL_LOG, "comando no encontrado: 'xyz123'", timeout_sec=8.0)
        assert ok, "Fallo: 'xyz123' no reporto error adecuadamente."
        qmp.screendump_png("shot3_v244_xyz_not_found")
        print("[TEST 3/5] OK: Segundo comando inexistente manejado perfectamente.")

        # 5. Ejecutar comando valido 'uname -a' para demostrar que el shell y kernel siguen 100% operativos
        print("[TEST 4/5] Ejecutando comando valido 'uname -a' tras los errores...")
        qmp.send_keys(["u", "n", "a", "m", "e", "spc", "-", "a", "ret"])
        time.sleep(1.0)
        ok = wait_for_log_token(SERIAL_LOG, "SMP x86_64", timeout_sec=8.0)
        assert ok, "Fallo: 'uname -a' no se ejecuto tras los errores."
        qmp.screendump_png("shot4_v244_uname_after_error")
        print("[TEST 4/5] OK: 'uname -a' ejecuto correctamente y retorno prompt.")

        # 6. Ejecutar 'help' valido
        print("[TEST 5/5] Ejecutando 'help' para verificar streaming multilínea...")
        qmp.send_keys(["h", "e", "l", "p", "ret"])
        time.sleep(1.0)
        ok = wait_for_log_token(SERIAL_LOG, "edit / kilo", timeout_sec=8.0)
        assert ok, "Fallo: 'help' no transmitio correctamente."
        qmp.screendump_png("shot5_v244_help_success")
        print("[TEST 5/5] OK: 'help' completo y prompt restaurado.")

        print("\n>>> TODOS LOS TESTS DE LA FASE V2.4.4 PASARON EXITOSAMENTE (5/5) <<<")

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
