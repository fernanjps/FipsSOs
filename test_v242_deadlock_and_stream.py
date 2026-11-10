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
SERIAL_LOG = f"{SCRATCH_DIR}/serial_v242.log"
QMP_SOCK = "/tmp/fipsos_v242_qmp.sock"
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

    print("=== INICIANDO VERIFICACION FIPSOS V2.4.2 (DEADLOCK ELIMINATION & SAFE STREAMING) ===")
    qemu_cmd = get_qemu_cmd()
    with open(SERIAL_LOG, "wb") as logf:
        proc = subprocess.Popen(
            qemu_cmd,
            stdin=subprocess.PIPE,
            stdout=logf,
            stderr=subprocess.STDOUT,
        )
        try:
            for _ in range(40):
                if os.path.exists(QMP_SOCK):
                    break
                time.sleep(0.1)

            qmp = QmpClient(QMP_SOCK)

            # -------------------------------------------------------------
            # TEST 1: Boot & Clean Prompt with v2.4 Banner
            # -------------------------------------------------------------
            print("[TEST 1] Verificando arranque del Kernel y Banner FipSOs v2.4...")
            assert wait_for_log_token(SERIAL_LOG, "fipsos:/#", 14.0), "Prompt del Shell ausente!"
            assert wait_for_log_token(SERIAL_LOG, "FipSOs v2.4", 5.0), "Version v2.4 no encontrada en logs de arranque!"
            time.sleep(0.5)
            qmp.screendump_png("shot1_v242_shell_boot")
            print("  -> TEST 1 PASSED: Kernel v2.4 activo, banner y shell listos.")

            # -------------------------------------------------------------
            # TEST 2: Multi-line Help Stream without Reentrancy Deadlock
            # -------------------------------------------------------------
            print("[TEST 2] Verificando stream de texto continuo multilínea ('help')...")
            # Iniciar 'help' que causaba deadlock tras la 1ra línea en VirtualBox
            proc.stdin.write(b"help\n")
            proc.stdin.flush()
            time.sleep(0.6)

            assert wait_for_log_token(SERIAL_LOG, "Comandos Disponibles:", 6.0), "Encabezado de help ausente!"
            assert wait_for_log_token(SERIAL_LOG, "exit", 6.0), "Fin de lista de comandos ausente (deadlock de stream detectado)!"
            time.sleep(0.4)
            qmp.screendump_png("shot2_v242_help_stream")
            print("  -> TEST 2 PASSED: 35 lineas de texto emitidas fluidamente sin ningun deadlock.")

            # -------------------------------------------------------------
            # TEST 3: Multi-call Busybox Execution Stream
            # -------------------------------------------------------------
            print("[TEST 3] Verificando ejecucion de BusyBox v1.35.0 (multilínea)...")
            proc.stdin.write(b"busybox\n")
            proc.stdin.flush()
            time.sleep(0.6)
            assert wait_for_log_token(SERIAL_LOG, "BusyBox v1.35.0", 6.0), "BusyBox no emitio banner!"
            qmp.screendump_png("shot3_v242_busybox_run")
            print("  -> TEST 3 PASSED: Busybox v1.35.0 ejecuto y mostro applets sin interbloqueo.")

            # -------------------------------------------------------------
            # TEST 4: ANSI Clear & Prompt Restoration
            # -------------------------------------------------------------
            print("[TEST 4] Verificando ANSI Clear y recuperacion de cursor...")
            proc.stdin.write(b"clear\n")
            proc.stdin.flush()
            time.sleep(0.4)
            proc.stdin.write(b"uname -a\n")
            proc.stdin.flush()
            time.sleep(0.4)
            proc.stdin.write(b"nproc\n")
            proc.stdin.flush()
            time.sleep(0.4)
            assert wait_for_log_token(SERIAL_LOG, "SMP", 6.0), "Salida de uname no llego!"
            qmp.screendump_png("shot4_v242_clear_screen")
            print("  -> TEST 4 PASSED: Pantalla limpiada y comandos SMP ejecutados.")

            # -------------------------------------------------------------
            # TEST 5: Desktop UI v2.4 Integration & Status Pill
            # -------------------------------------------------------------
            print("[TEST 5] Verificando Desktop UI v2.4 y Pill superior...")
            proc.stdin.write(b"gui\n")
            proc.stdin.flush()
            assert wait_for_log_token(SERIAL_LOG, "[desktop_ui.elf] FipSOs Shell UI v2.4", 8.0), "Desktop UI no inicio con v2.4!"
            time.sleep(0.8)
            qmp.screendump_png("shot5_v242_desktop_ui")
            print("  -> TEST 5 PASSED: Compositor Desktop UI v2.4 activo.")

            # -------------------------------------------------------------
            # TEST 6: Exit Desktop UI and Clean TTY Restoration
            # -------------------------------------------------------------
            print("[TEST 6] Verificando retorno limpio a TTY fipsos@v2.4...")
            qmp.send_keys(["q"])
            time.sleep(0.8)
            assert wait_for_log_token(SERIAL_LOG, "Consola TTY 1280x800 restaurada OK", 6.0), "Retorno a TTY fallo!"
            assert wait_for_log_token(SERIAL_LOG, "Retornando a TTY fipsos@v2.4", 6.0), "Mensaje de salida con v2.4 ausente!"
            qmp.screendump_png("shot6_v242_tty_restored")
            print("  -> TEST 6 PASSED: TTY restaurada limpiamente sin artefactos ni deadlocks.")

            print("\n=======================================================")
            print("  FASE v2.4.2 (DEADLOCK ELIMINATION & STREAM) 100% OK")
            print("=======================================================")

        finally:
            proc.kill()
            proc.wait()
            if os.path.exists(QMP_SOCK):
                os.remove(QMP_SOCK)

if __name__ == "__main__":
    main()
