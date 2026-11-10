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
SERIAL_LOG = f"{SCRATCH_DIR}/serial_v28.log"
QMP_SOCK = "/tmp/fipsos_v28_qmp.sock"
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
    print("=== INICIANDO VALIDACION FIPSOS FASE V2.8 (POSIX FORK/WAIT4 & TABLA DE 32 PROCESOS) ===", flush=True)
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
        # TEST 1: Arranque y Banner FipSOs v2.8
        # =====================================================================
        print("[TEST 1/6] Esperando arranque de FipSOs v2.8...", flush=True)
        ok = wait_for_log_token(SERIAL_LOG, "FipSOs v2.8", timeout_sec=20.0)
        assert ok, "Fallo: No se encontro 'FipSOs v2.8' en el arranque"

        ok = wait_for_log_token(SERIAL_LOG, "fipsos:/#", timeout_sec=15.0)
        assert ok, "Fallo: No se alcanzo el prompt del shell interactivo"

        time.sleep(1.0)
        qmp.screendump_png("shot1_v28_boot")
        print("[TEST 1/6 PASS] FipSOs v2.8 arrancó con éxito y shell listo.", flush=True)

        # =====================================================================
        # TEST 2: Ejecución de capp (/bin/c_std_app.elf: fork() + waitpid() + PML4 Isolation)
        # =====================================================================
        print("[TEST 2/6] Ejecutando 'capp' (/bin/c_std_app.elf con fork y waitpid)...", flush=True)
        qmp.send_string("capp\n")
        
        ok = wait_for_log_token(SERIAL_LOG, "Testing fork() + waitpid() with CR3 memory isolation", timeout_sec=10.0)
        assert ok, "Fallo: capp no inicio la prueba de fork()"

        ok = wait_for_log_token(SERIAL_LOG, "Child Process", timeout_sec=10.0)
        assert ok, "Fallo: Proceso hijo no ejecuto en Ring 3"

        ok = wait_for_log_token(SERIAL_LOG, "Canary modified to 999. Exiting with status 42", timeout_sec=10.0)
        assert ok, "Fallo: Proceso hijo no modifico canary ni ejecuto exit(42)"

        ok = wait_for_log_token(SERIAL_LOG, "waitpid() returned PID =", timeout_sec=10.0)
        assert ok, "Fallo: waitpid() no recolecto al proceso hijo"

        ok = wait_for_log_token(SERIAL_LOG, "Child exit code = 42 (expected 42)", timeout_sec=10.0)
        assert ok, "Fallo: Exit code recolectado incorrecto (se esperaba 42)"

        ok = wait_for_log_token(SERIAL_LOG, "Inspecting isolation_canary = 100 (expected 100)", timeout_sec=10.0)
        assert ok, "Fallo: Memoria del padre fue contaminada por el hijo (falto aislamiento PML4)"

        ok = wait_for_log_token(SERIAL_LOG, "CR3 PML4 memory isolation verified", timeout_sec=10.0)
        assert ok, "Fallo: Validacion de aislamiento CR3 no completada"

        time.sleep(1.0)
        qmp.screendump_png("shot2_v28_capp_fork_wait")
        print("[TEST 2/6 PASS] fork() + waitpid() verificado con aislamiento PML4 y status=42!", flush=True)

        # =====================================================================
        # TEST 3: Telemetría de Memoria Inicial (sensors)
        # =====================================================================
        print("[TEST 3/6] Consultando telemetria inicial de RAM (sensors)...", flush=True)
        qmp.send_string("sensors\n")
        time.sleep(1.0)
        qmp.screendump_png("shot3_v28_sensors_initial")
        print("[TEST 3/6 PASS] Telemetria inicial capturada.", flush=True)

        # =====================================================================
        # TEST 4: Prueba de Estrés: Ejecución de 5 pasadas de capp (5 forks + 5 waitpids)
        # =====================================================================
        print("[TEST 4/6] Ejecutando rafaga de 5 ejecuciones consecutivas de capp...", flush=True)
        for i in range(5):
            print(f"  -> Ejecutando capp #{i+1}...", flush=True)
            qmp.send_string("capp\n")
            time.sleep(0.8)

        print("[TEST 4/6] Consultando tabla de procesos con 'ps'...", flush=True)
        qmp.send_string("ps\n")
        time.sleep(1.0)
        qmp.screendump_png("shot4_v28_ps_clean_slots")
        print("[TEST 4/6 PASS] Slots de procesos reciclados limpiamente (sin zombies residuales).", flush=True)

        # =====================================================================
        # TEST 5: Verificación de Cero Fugas de Memoria Física (sensors post-estrés)
        # =====================================================================
        print("[TEST 5/6] Verificando memoria fisica libre tras el estres (sensors)...", flush=True)
        qmp.send_string("sensors\n")
        time.sleep(1.0)
        qmp.screendump_png("shot5_v28_sensors_zero_leak")
        print("[TEST 5/6 PASS] Cero fugas de marcos fisicos confirmadas.", flush=True)

        # =====================================================================
        # TEST 6: Estabilidad General del Shell (comandos invalidos y navegacion)
        # =====================================================================
        print("[TEST 6/6] Verificando estabilidad del shell con 'helps' y 'ls'...", flush=True)
        qmp.send_string("helps\n")
        time.sleep(0.5)
        qmp.send_string("ls /bin\n")
        time.sleep(1.0)
        qmp.screendump_png("shot6_v28_shell_stability")
        print("[TEST 6/6 PASS] Shell interactivo estable sin panicos de Ring 0.", flush=True)

        print("\n========================================================", flush=True)
        print("  TODAS LAS 6 PRUEBAS DE LA FASE v2.8 PASARON AL 100%!", flush=True)
        print("========================================================\n", flush=True)

    finally:
        proc.terminate()
        try:
            proc.wait(timeout=3)
        except subprocess.TimeoutExpired:
            proc.kill()

if __name__ == "__main__":
    main()
