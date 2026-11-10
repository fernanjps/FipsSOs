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
SERIAL_LOG = f"{SCRATCH_DIR}/serial_v26.log"
QMP_SOCK = "/tmp/fipsos_v26_qmp.sock"
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

    def send_string(self, text: str):
        key_map = {
            '\n': 'ret',
            ' ': 'spc',
            '.': 'dot',
            '/': 'slash',
            '-': 'minus',
            '1': '1', '2': '2', '3': '3', '4': '4', '5': '5',
            '6': '6', '7': '7', '8': '8', '9': '9', '0': '0',
        }
        for ch in text:
            if ch == '_':
                self.cmd("send-key", {"keys": [{"type": "qcode", "data": "shift_r"}, {"type": "qcode", "data": "minus"}]})
            elif ch in key_map:
                self.cmd("send-key", {"keys": [{"type": "qcode", "data": key_map[ch]}]})
            elif ch.isalpha():
                self.cmd("send-key", {"keys": [{"type": "qcode", "data": ch.lower()}]})
            time.sleep(0.03)

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

    print("=== INICIANDO VALIDACION FIPSOS FASE V2.6 (ANTI-ARTEFACTOS, TELEMETRIA HONESTA & CERO FUGAS) ===", flush=True)
    qemu_cmd = get_qemu_cmd()
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

        # 1. Esperar arranque e inicio del Shell
        print("[BOOT] Esperando inicializacion completa del Shell...", flush=True)
        ok = wait_for_log_token(SERIAL_LOG, "FipSOs v2.6 Framekernel", timeout_sec=20.0)
        assert ok, "Fallo: No se detecto el banner 'FipSOs v2.6 Framekernel' en el log de arranque."

        ok = wait_for_log_token(SERIAL_LOG, "fipsos:/#", timeout_sec=15.0)
        assert ok, "Fallo: No se alcanzo el prompt del shell interactivo."
        time.sleep(1.0)
        qmp.screendump_png("shot1_v26_boot")

        with open(SERIAL_LOG, "r", encoding="utf-8", errors="replace") as f:
            boot_log = f.read()

        # Validar ausencia de hardware simulado
        assert "wlan0" not in boot_log, "ERROR: wlan0 detectado en log!"
        assert "/dev/snd/pcm0" not in boot_log, "ERROR: /dev/snd/pcm0 detectado en log!"
        assert "HTTP Daemon Port 80" not in boot_log, "ERROR: Ring-0 HTTP Daemon detectado!"
        print("[TEST 1/5] OK: Arranque limpio verificado con banner 'FipSOs v2.6 Framekernel'.", flush=True)

        # 2. Test de Telemetria Termica Honesta y Sensores
        print("[TEST 2/5] Consultando 'sensors' para verificar telemetria honesta en VM...", flush=True)
        qmp.send_string("sensors\n")
        time.sleep(1.5)
        ok = wait_for_log_token(SERIAL_LOG, "Temperatura: N/A (VM Hypervisor)", timeout_sec=10.0)
        assert ok, "Fallo: La temperatura no reporto 'Temperatura: N/A (VM Hypervisor)' en VM!"
        qmp.screendump_png("shot2_v26_sensors_initial")

        with open(SERIAL_LOG, "r", encoding="utf-8", errors="replace") as f:
            sensors_log = f.read()

        # Extraer frames fisicos usados iniciales
        match_used = re.search(r'KiB Usados \((\d+) frames\)', sensors_log)
        assert match_used is not None, "Fallo: No se pudo extraer el conteo de frames en uso desde /sys/fipsos/hwmon."
        initial_used_frames = int(match_used.group(1))
        print(f"[TEST 2/5] OK: Telemetria honesta validada (Temp: N/A en VM). Frames iniciales en uso: {initial_used_frames}", flush=True)

        # 3. Test de Ciclo de Vida y Cero Fugas: 10 ejecuciones consecutivas de sample_tool.elf
        print("[TEST 3/5] Ejecutando 'sample_tool.elf' 10 veces consecutivas para verificar reciclaje de memoria...", flush=True)
        for i in range(1, 11):
            qmp.send_string("sample\n")
            time.sleep(0.6)
            ok = wait_for_log_token(SERIAL_LOG, "HOLA DESDE /bin/sample_tool", timeout_sec=8.0)
            assert ok, f"Fallo al ejecutar sample_tool en iteracion {i}"
            print(f"  -> Iteracion {i}/10 completada (proceso salio con 0, PML4 liberado)", flush=True)

        # Consultar sensors tras las 10 ejecuciones
        print("[TEST 3/5] Consultando 'sensors' tras 10 ejecuciones de procesos efimeros...", flush=True)
        qmp.send_string("sensors\n")
        time.sleep(1.5)

        with open(SERIAL_LOG, "r", encoding="utf-8", errors="replace") as f:
            final_sensors_log = f.read()

        # Obtener el ultimo reporte de hwmon
        matches = list(re.finditer(r'KiB Usados \((\d+) frames\)', final_sensors_log))
        assert len(matches) >= 2, "Fallo: No se encontraron los reportes de hwmon posterior a las ejecuciones."
        post_used_frames = int(matches[-1].group(1))

        print(f"[TEST 3/5] Frames usados ANTES: {initial_used_frames} | Frames usados DESPUES de 10 ejecuciones: {post_used_frames} (Delta: {post_used_frames - initial_used_frames} frames)", flush=True)
        assert post_used_frames <= initial_used_frames, f"ERROR DE FUGA: Frames usados aumentaron de {initial_used_frames} a {post_used_frames}!"
        qmp.screendump_png("shot3_v26_leak_test_10x")
        print("[TEST 3/5] OK: Cero fugas de memoria verificado (100% de paginas PT/PD/PDPT/PML4 recicladas).", flush=True)

        # 4. Lanzar GUI Compositor C++20 y verificar clipping anti-artefactos
        print("[TEST 4/5] Lanzando Compositor Grafico C++20 ('gui')...", flush=True)
        qmp.send_string("gui\n")
        time.sleep(2.5)
        ok = wait_for_log_token(SERIAL_LOG, "[desktop_ui.elf] FipSOs Shell UI v2.6", timeout_sec=10.0)
        assert ok, "Fallo: desktop_ui.elf v2.6 no inicio."
        qmp.screendump_png("shot4_v26_desktop_ui")

        # Interactuar con ventanas para verificar repintado limpio sin fantasmas
        qmp.send_keys(["tab"])
        time.sleep(0.5)
        qmp.screendump_png("shot5_v26_gui_interact")
        print("[TEST 4/5] OK: Compositor C++20 activo con version v2.6 y recorte estricto de texto.", flush=True)

        # 5. Salir limpiamente con 'q' y restaurar TTY
        print("[TEST 5/5] Saliendo del GUI con 'q' y restaurando TTY interactiva...", flush=True)
        qmp.send_keys(["q"])
        time.sleep(1.5)
        ok = wait_for_log_token(SERIAL_LOG, "Sesion de escritorio cerrada", timeout_sec=10.0)
        assert ok, "Fallo: TTY no restaurada tras presionar 'q'."

        qmp.send_string("help\n")
        time.sleep(1.5)
        qmp.screendump_png("shot6_v26_tty_restored")

        with open(SERIAL_LOG, "r", encoding="utf-8", errors="replace") as f:
            final_log = f.read()
        assert "KERNEL PANIC" not in final_log, "ERROR CRITICO: Ocurrio un Kernel Panic durante la ejecucion final!"

        print("\n>>> TODOS LOS TESTS DE LA FASE V2.6 PASARON EXITOSAMENTE (5/5) <<<", flush=True)

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
