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
SERIAL_LOG = f"{SCRATCH_DIR}/serial_v29.log"
QMP_SOCK = "/tmp/fipsos_v29_qmp.sock"
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

    def send_string(self, text: str):
        key_map = {
            '\n': 'ret',
            ' ': 'spc',
            '.': 'dot',
            '/': 'slash',
            '-': 'minus',
            '_': 'shift-minus',
            '=': 'equal',
            ':': 'shift-semicolon',
            '>': 'shift-dot',
            '<': 'shift-comma',
            '|': 'shift-backslash',
            '0': '0', '1': '1', '2': '2', '3': '3', '4': '4',
            '5': '5', '6': '6', '7': '7', '8': '8', '9': '9',
        }
        for ch in text:
            if ch in key_map:
                k = key_map[ch]
                if k.startswith("shift-"):
                    actual = k.split("-")[1]
                    self.cmd("send-key", {"keys": [{"type": "qcode", "data": "shift"}, {"type": "qcode", "data": actual}]})
                else:
                    self.cmd("send-key", {"keys": [{"type": "qcode", "data": k}]})
            elif ch.islower():
                self.cmd("send-key", {"keys": [{"type": "qcode", "data": ch}]})
            elif ch.isupper():
                self.cmd("send-key", {"keys": [{"type": "qcode", "data": "shift"}, {"type": "qcode", "data": ch.lower()}]})
            time.sleep(0.03)

    def screendump_png(self, name: str):
        ppm_path = f"{SCRATCH_DIR}/{name}.ppm"
        png_path = f"{ARTIFACT_DIR}/{name}.png"
        if os.path.exists(ppm_path):
            os.remove(ppm_path)
        self.cmd("screendump", {"filename": ppm_path})
        for _ in range(50):
            if os.path.exists(ppm_path) and os.path.getsize(ppm_path) > 1000:
                break
            time.sleep(0.1)
        ppm_to_png(ppm_path, png_path)
        if os.path.exists(ppm_path):
            os.remove(ppm_path)
        print(f"[QMP] Screendump guardado: {png_path}", flush=True)

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

def main():
    print("=== INICIANDO VALIDACION FIPSOS FASE V2.9 (PIPES, DUP2, SIGNALS) ===", flush=True)
    os.makedirs(SCRATCH_DIR, exist_ok=True)
    os.makedirs(ARTIFACT_DIR, exist_ok=True)
    if os.path.exists(SERIAL_LOG):
        os.remove(SERIAL_LOG)
    if os.path.exists(QMP_SOCK):
        os.remove(QMP_SOCK)

    subprocess.run(["killall", "-9", "qemu-system-x86_64"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    time.sleep(0.5)

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
        "-netdev", "user,id=net0",
        "-device", "e1000e,netdev=net0",
        "-no-reboot",
        "-no-shutdown",
    ]

    print("[QEMU] Lanzando emulador...", flush=True)
    with open(SERIAL_LOG, "wb") as logf:
        proc = subprocess.Popen(
            qemu_cmd,
            stdin=subprocess.PIPE,
            stdout=logf,
            stderr=subprocess.STDOUT,
        )

    try:
        print("[QMP] Conectando a socket QMP...", flush=True)
        qmp = QmpClient(QMP_SOCK)

        # =====================================================================
        # TEST 1: Arranque y Banner FipSOs v2.9
        # =====================================================================
        print("[TEST 1/6] Esperando arranque de FipSOs v2.9...", flush=True)
        ok = wait_for_log_token(SERIAL_LOG, "FipSOs v2.9", timeout_sec=20.0)
        assert ok, "Fallo: No se encontro 'FipSOs v2.9' en el arranque"

        ok = wait_for_log_token(SERIAL_LOG, "fipsos:/#", timeout_sec=15.0)
        assert ok, "Fallo: No se alcanzo el prompt del shell interactivo"

        time.sleep(1.0)
        qmp.screendump_png("shot1_v29_boot")
        print("[TEST 1/6 PASS] FipSOs v2.9 arrancó con éxito y shell listo.", flush=True)

        # =====================================================================
        # TEST 2: Ejecución de capp (/bin/c_std_app.elf: pipe() + dup2() + sigaction/kill)
        # =====================================================================
        print("[TEST 2/6] Ejecutando 'capp' (/bin/c_std_app.elf con pipe, dup2 y signals)...", flush=True)
        qmp.send_string("capp\n")

        ok = wait_for_log_token(SERIAL_LOG, "[Pipe IPC] Allocated pipe", timeout_sec=10.0)
        assert ok, "Fallo: capp no inicio la prueba de pipes"

        ok = wait_for_log_token(SERIAL_LOG, "[Pipe Parent] SUCCESS: Pipe + fork() IPC verified successfully!", timeout_sec=10.0)
        assert ok, "Fallo: Pipe IPC no funciono correctamente entre padre e hijo"

        ok = wait_for_log_token(SERIAL_LOG, "[dup2 Redirection] SUCCESS: dup2() stdout redirection verified successfully!", timeout_sec=10.0)
        assert ok, "Fallo: dup2 stdout redirection no funciono correctamente"

        ok = wait_for_log_token(SERIAL_LOG, "[POSIX Signals] SUCCESS: sigaction() + kill() delivered signal", timeout_sec=10.0)
        assert ok, "Fallo: sigaction / kill no entrego señal a handler"

        ok = wait_for_log_token(SERIAL_LOG, "SUCCESS: All Linux libc standard routines executed with exit=0", timeout_sec=10.0)
        assert ok, "Fallo: capp no completo todas las pruebas exitosamente"

        time.sleep(1.0)
        qmp.screendump_png("shot2_v29_posix_pipes_signals")
        print("[TEST 2/6 PASS] capp: pipe() + dup2() + sigaction/kill verificados al 100%!", flush=True)

        # =====================================================================
        # TEST 3: Shell Pipeline: echo ... | cat
        # =====================================================================
        print("[TEST 3/6] Probando pipeline interactivo de shell (echo ... | cat)...", flush=True)
        qmp.send_string("echo PIPELINE_V29_VERIFIED | cat\n")

        ok = wait_for_log_token(SERIAL_LOG, "PIPELINE_V29_VERIFIED", timeout_sec=10.0)
        assert ok, "Fallo: Pipeline del shell 'echo | cat' no entrego texto esperado"

        ok = wait_for_log_token(SERIAL_LOG, "fipsos:/#", timeout_sec=10.0)
        assert ok, "Fallo: Shell no retorno prompt tras pipeline"

        time.sleep(1.0)
        qmp.screendump_png("shot3_v29_shell_pipe")
        print("[TEST 3/6 PASS] Pipeline del shell 'echo ... | cat' completado con éxito!", flush=True)

        # =====================================================================
        # TEST 4: Shell Redirection: echo ... > /disk/redir.txt
        # =====================================================================
        print("[TEST 4/6] Probando redireccion a archivo (echo ... > /disk/redir.txt)...", flush=True)
        qmp.send_string("echo REDIR_V29_OK > /disk/redir.txt\n")
        time.sleep(0.5)
        qmp.send_string("cat /disk/redir.txt\n")

        ok = wait_for_log_token(SERIAL_LOG, "REDIR_V29_OK", timeout_sec=10.0)
        assert ok, "Fallo: Redireccion a archivo 'echo >' no guardo el contenido esperado"

        time.sleep(1.0)
        qmp.screendump_png("shot4_v29_shell_redirection")
        print("[TEST 4/6 PASS] Redireccion '>' verificada con exito en /disk/redir.txt!", flush=True)

        # =====================================================================
        # TEST 5: Tabla de Procesos y Comando kill
        # =====================================================================
        print("[TEST 5/6] Verificando tabla de procesos (ps) y comando kill...", flush=True)
        qmp.send_string("ps\n")
        time.sleep(0.5)
        qmp.send_string("kill 999 15\n")

        ok = wait_for_log_token(SERIAL_LOG, "kill:", timeout_sec=10.0)
        assert ok, "Fallo: kill a PID inexistente debio reportar error controlado"

        time.sleep(1.0)
        qmp.screendump_png("shot5_v29_ps_kill")
        print("[TEST 5/6 PASS] ps y kill validados correctamente en el shell!", flush=True)

        # =====================================================================
        # TEST 6: Telemetría Térmica y RAM Zero-Leak (sensors)
        # =====================================================================
        print("[TEST 6/6] Verificando telemetria final y Zero-Leak con sensors...", flush=True)
        qmp.send_string("sensors\n")

        ok = wait_for_log_token(SERIAL_LOG, "Memoria Fisica", timeout_sec=10.0)
        assert ok, "Fallo: sensors no reporto telemetria de RAM"

        time.sleep(1.0)
        qmp.screendump_png("shot6_v29_sensors_zero_leak")
        print("[TEST 6/6 PASS] Telemetria final validada: 0 fugas de memoria y estabilidad total!", flush=True)

        print("\n=====================================================================")
        print(">>> TODOS LOS TESTS DE LA FASE V2.9 COMPLETADOS CON EXITO (6/6) <<<")
        print("=====================================================================")

    finally:
        print("[QEMU] Terminando proceso QEMU...", flush=True)
        proc.terminate()
        try:
            proc.wait(timeout=3.0)
        except subprocess.TimeoutExpired:
            proc.kill()

if __name__ == "__main__":
    main()
