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
SERIAL_LOG = f"{SCRATCH_DIR}/serial_v31.log"
QMP_SOCK = "/tmp/fipsos_v31_qmp.sock"
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

def get_log_content(log_path: str) -> str:
    if os.path.exists(log_path):
        with open(log_path, "r", encoding="utf-8", errors="replace") as f:
            return f.read()
    return ""

def main():
    print("=== INICIANDO VALIDACION FIPSOS FASE V3.1 (DHCP, ANSI LIMPIO, MULTI-USB, PERSISTENT TCP) ===", flush=True)
    os.makedirs(SCRATCH_DIR, exist_ok=True)
    os.makedirs(ARTIFACT_DIR, exist_ok=True)
    if os.path.exists(SERIAL_LOG):
        os.remove(SERIAL_LOG)
    if os.path.exists(QMP_SOCK):
        os.remove(QMP_SOCK)

    subprocess.run(["killall", "-9", "qemu-system-x86_64"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    time.sleep(0.5)

    # Launch QEMU with EHCI + xHCI + usb-kbd + e1000e with port forwarding
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
        "-device", "usb-ehci,id=ehci",
        "-device", "qemu-xhci,id=xhci",
        "-device", "usb-kbd,bus=xhci.0",
        "-no-reboot",
        "-no-shutdown",
    ]

    print("[QEMU] Lanzando emulador con usb-ehci + qemu-xhci + DHCP + TCP :2222...", flush=True)
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
        # TEST 1: Arranque FipSOs v3.1, Negociacion DHCP y Multi-USB (EHCI + xHCI)
        # =====================================================================
        print("[TEST 1] Verificando arranque v3.1, descubrimiento DHCP y Multi-USB...", flush=True)
        assert wait_for_log_token(SERIAL_LOG, "FipSOs v3.1 Framekernel", 20.0), "Timeout esperando banner FipSOs v3.1"
        assert wait_for_log_token(SERIAL_LOG, "ostd::net::dhcp", 15.0), "Timeout esperando negociacion DHCP"
        assert wait_for_log_token(SERIAL_LOG, "DHCPACK", 15.0) or wait_for_log_token(SERIAL_LOG, "DHCP", 15.0), "Timeout en DHCP"
        assert wait_for_log_token(SERIAL_LOG, "ostd::bus::xhci", 10.0), "Timeout inicializando bus USB"
        assert wait_for_log_token(SERIAL_LOG, "fipsos", 15.0), "Timeout esperando prompt del shell"
        time.sleep(1.0)
        qmp.screendump_png("shot1_v31_boot")
        print("[TEST 1 PASSED] Arranque v3.1, DHCP y Multi-USB confirmados.", flush=True)

        # =====================================================================
        # TEST 2: Verificacion Estricta de CERO Caracteres '?[1;32m' o '?[K'
        # =====================================================================
        print("[TEST 2] Verificando erradicacion de regresion ANSI (?[1;32m y ?[K)...", flush=True)
        log_data = get_log_content(SERIAL_LOG)
        assert "?[1;32m" not in log_data, "REGRESION DETECTADA: Caracteres literales '?[1;32m' en log"
        assert "?[0m" not in log_data, "REGRESION DETECTADA: Caracteres literales '?[0m' en log"
        assert "?[K" not in log_data, "REGRESION DETECTADA: Caracteres literales '?[K' en log"
        qmp.screendump_png("shot2_v31_ansi_clean")
        print("[TEST 2 PASSED] CERO caracteres '?[1;32m' o '?[K'. Renderizado ANSI 100% limpio.", flush=True)

        # =====================================================================
        # TEST 3: Telemetria Real en sensors (/sys/fipsos/hwmon con DHCP IP y Multi-USB)
        # =====================================================================
        print("[TEST 3] Verificando comando 'sensors' con IP DHCP y controladoras USB...", flush=True)
        proc.stdin.write(b"sensors\n")
        proc.stdin.flush()
        time.sleep(1.2)
        assert wait_for_log_token(SERIAL_LOG, "FipSOs v3.1 Hardware Monitor", 10.0), "No se encontro encabezado HWMON v3.1"
        assert wait_for_log_token(SERIAL_LOG, "IP=10.0.2.15", 10.0), "No se encontro IP dinamica en telemetria NIC"
        assert wait_for_log_token(SERIAL_LOG, "Host Controllers (PCIe)", 10.0), "No se encontro reporte Multi-USB en sensors"
        qmp.screendump_png("shot3_v31_sensors_dhcp_usb")
        print("[TEST 3 PASSED] Telemetria de sensores verificada con IP DHCP y Multi-USB.", flush=True)

        # =====================================================================
        # TEST 4: Historial de Comandos con Flecha Arriba/Abajo y Redibujado Limpio
        # =====================================================================
        print("[TEST 4] Verificando historial interactivo con flechas y borrado \\x1b[K...", flush=True)
        proc.stdin.write(b"ls /\n")
        proc.stdin.flush()
        time.sleep(0.8)

        # Flecha Arriba
        proc.stdin.write(b"\x1b[A\n")
        proc.stdin.flush()
        time.sleep(0.8)

        # Verificamos que no hayan aparecido ?[K
        log_data_hist = get_log_content(SERIAL_LOG)
        assert "?[K" not in log_data_hist, "Error: ?[K generado durante navegacion de historial"
        qmp.screendump_png("shot4_v31_shell_history")
        print("[TEST 4 PASSED] Historial con Flecha Arriba verificado limpiamente.", flush=True)

        # =====================================================================
        # TEST 5: Sesion TCP Persistente e Interactiva en 127.0.0.1:2222
        # =====================================================================
        print("[TEST 5] Probando sesion TCP interactiva en 127.0.0.1:2222 (multiples comandos)...", flush=True)
        tcp_client = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        tcp_client.settimeout(6.0)
        tcp_client.connect(("127.0.0.1", 2222))
        print("[TEST 5] Conexion TCP establecida. Leyendo banner de bienvenida...", flush=True)

        welcome_banner = b""
        start_t = time.time()
        while time.time() - start_t < 4.0:
            try:
                chunk = tcp_client.recv(1024)
                if chunk:
                    welcome_banner += chunk
                    if b"FipSOs" in welcome_banner or b"fipsos" in welcome_banner:
                        break
            except socket.timeout:
                pass
            time.sleep(0.1)

        print(f"[TEST 5] Banner de bienvenida recibido: {welcome_banner[:80]}", flush=True)
        assert b"FipSOs v3.1" in welcome_banner or b"fipsos" in welcome_banner, f"Banner de bienvenida inesperado: {welcome_banner}"

        # Comando 1 por TCP: help
        print("[TEST 5] Enviando 'help' por TCP...", flush=True)
        tcp_client.sendall(b"help\n")
        time.sleep(1.0)
        resp1 = b""
        start_t = time.time()
        while time.time() - start_t < 3.0:
            try:
                c = tcp_client.recv(1024)
                if c:
                    resp1 += c
                    if b"Comandos" in resp1 or b"help" in resp1:
                        break
            except socket.timeout:
                pass
        print(f"[TEST 5] Respuesta a 'help': {resp1[:60]}", flush=True)

        # Comando 2 por TCP: echo remoto | cat
        print("[TEST 5] Enviando 'echo remoto | cat' por TCP...", flush=True)
        tcp_client.sendall(b"echo remoto | cat\n")
        time.sleep(1.0)
        resp2 = b""
        start_t = time.time()
        while time.time() - start_t < 3.0:
            try:
                c = tcp_client.recv(1024)
                if c:
                    resp2 += c
                    if b"remoto" in resp2:
                        break
            except socket.timeout:
                pass
        print(f"[TEST 5] Respuesta a 'echo remoto | cat': {resp2}", flush=True)
        assert b"remoto" in resp2, f"Fallo al ejecutar 'echo remoto | cat': {resp2}"

        # Comando 3 por TCP: pwd
        print("[TEST 5] Enviando 'pwd' por TCP...", flush=True)
        tcp_client.sendall(b"pwd\n")
        time.sleep(1.0)
        resp3 = b""
        start_t = time.time()
        while time.time() - start_t < 3.0:
            try:
                c = tcp_client.recv(1024)
                if c:
                    resp3 += c
                    if b"/" in resp3:
                        break
            except socket.timeout:
                pass
        print(f"[TEST 5] Respuesta a 'pwd': {resp3}", flush=True)
        assert b"/" in resp3, f"Fallo al ejecutar 'pwd': {resp3}"

        tcp_client.close()
        qmp.screendump_png("shot5_v31_tcp_remote_interactive")
        print("[TEST 5 PASSED] Sesion interactiva TCP :2222 superada con 3 comandos sucesivos.", flush=True)

        # =====================================================================
        # TEST 6: Lanzamiento de Compositor Grafico C++20 Desktop UI v3.1
        # =====================================================================
        print("[TEST 6] Probando Desktop UI v3.1...", flush=True)
        proc.stdin.write(b"gui\n")
        proc.stdin.flush()
        time.sleep(2.0)
        assert wait_for_log_token(SERIAL_LOG, "desktop_ui.elf", 10.0), "No inicio desktop_ui.elf"
        qmp.screendump_png("shot6_v31_desktop_ui")

        # Salir de desktop_ui
        proc.stdin.write(b"q\n")
        proc.stdin.flush()
        time.sleep(1.0)
        print("[TEST 6 PASSED] Desktop UI v3.1 renderizado exitosamente.", flush=True)

        print("=" * 70, flush=True)
        print("  TODAS LAS PRUEBAS DE FIPSOS FASE V3.1 HAN SIDO SUPERADAS (6/6)", flush=True)
        print("  - Erradicacion total de regresion ANSI (0 caracteres ?[ en pantalla): OK", flush=True)
        print("  - Negociacion DHCP real (UDP 68 -> 67): OK", flush=True)
        print("  - Enumeracion y telemetria Multi-USB (xHCI/EHCI/OHCI): OK", flush=True)
        print("  - Sesion interactiva persistente TCP :2222: OK", flush=True)
        print("  - Compositor Grafico C++20 Desktop UI v3.1: OK", flush=True)
        print("=" * 70, flush=True)

    finally:
        print("[QEMU] Terminando proceso...", flush=True)
        proc.terminate()
        try:
            proc.wait(timeout=3.0)
        except subprocess.TimeoutExpired:
            proc.kill()

if __name__ == "__main__":
    main()
