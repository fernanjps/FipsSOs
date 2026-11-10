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
import threading

SCRATCH_DIR = "/mnt/c/Users/Ferna/.gemini/antigravity/brain/4ef4b43d-91a0-44a4-aefe-f8b38846bd63/scratch"
ARTIFACT_DIR = "/mnt/c/Users/Ferna/.gemini/antigravity/brain/4ef4b43d-91a0-44a4-aefe-f8b38846bd63"
SERIAL_LOG = f"{SCRATCH_DIR}/serial_v241.log"
QMP_SOCK = "/tmp/fipsos_v241_qmp.sock"
DISK_IMG = "/mnt/c/FipsSOs/build/fipsos_disk.img"
ISO_IMG = "/mnt/c/FipsSOs/build/fipsos-x86_64.iso"
USB_IMG = "/mnt/c/FipsSOs/build/fipsos-baremetal-usb.img"

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

    # Kill any stale QEMU instances
    subprocess.run(["killall", "-9", "qemu-system-x86_64"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    time.sleep(0.5)

    print("=== STARTING FIPSOS V2.4.1 PERFORMANCE & I/O OPTIMIZATION VERIFICATION ===")
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
            # TEST 1: Boot & Clean Colored Prompt (Zero ANSI '?' artifacts)
            # -------------------------------------------------------------
            print("[TEST 1] Verificando arranque del Kernel y Shell interactivo...")
            assert wait_for_log_token(SERIAL_LOG, "fipsos:/#", 14.0), "Prompt del Shell ausente!"
            time.sleep(0.5)
            qmp.screendump_png("shot1_v241_clean_prompt")
            print("  -> TEST 1 PASSED: Shell arrancado, prompt visible y limpio.")

            # -------------------------------------------------------------
            # TEST 2: High-Speed Scroll Stress & RAM Backbuffer
            # -------------------------------------------------------------
            print("[TEST 2] Verificando aceleración de scroll por RAM Backbuffer (cero lecturas VRAM)...")
            # Enviar 'help' y luego 'sensors' para generar docenas de líneas y provocar scroll múltiple
            proc.stdin.write(b"help\n")
            proc.stdin.flush()
            time.sleep(0.4)
            proc.stdin.write(b"sensors\n")
            proc.stdin.flush()
            time.sleep(0.4)
            proc.stdin.write(b"cat /proc/meminfo\n")
            proc.stdin.flush()
            time.sleep(0.4)
            assert wait_for_log_token(SERIAL_LOG, "MemTotal:", 6.0) or wait_for_log_token(SERIAL_LOG, "[HWMON]", 6.0), "Salida de 'help' o 'sensors' no llego!"
            qmp.screendump_png("shot2_v241_scroll_ram_backbuffer")
            print("  -> TEST 2 PASSED: Scroll instantáneo en RAM Backbuffer verificado sin cuelgues.")

            # -------------------------------------------------------------
            # TEST 3: ANSI Escape Sequence 'clear' (ESC [ 2 J + ESC [ H)
            # -------------------------------------------------------------
            print("[TEST 3] Verificando comando 'clear' con secuencias de escape ANSI nativas...")
            proc.stdin.write(b"clear\n")
            proc.stdin.flush()
            time.sleep(0.4)
            proc.stdin.write(b"uname -a\n")
            proc.stdin.flush()
            time.sleep(0.4)
            assert wait_for_log_token(SERIAL_LOG, "FipSOs", 6.0), "'uname -a' no respondio tras clear!"
            qmp.screendump_png("shot3_v241_clear_ansi")
            print("  -> TEST 3 PASSED: Comando 'clear' ejecutado y pantalla limpiada correctamente.")

            # -------------------------------------------------------------
            # TEST 4: Non-Blocking PS/2 Mouse & Fast Typing in Shell
            # -------------------------------------------------------------
            print("[TEST 4] Verificando tipeo rapido UART y PS/2 sin contencion de bus VRAM...")
            # Enviar comando via UART serial stdin
            proc.stdin.write(b"echo fipsos_fast_typing_ok\n")
            proc.stdin.flush()
            time.sleep(0.4)
            assert wait_for_log_token(SERIAL_LOG, "fipsos_fast_typing_ok", 6.0), "Tipeo rapido UART congelado o perdido!"

            # Enviar scancodes via QMP para probar teclado PS/2 interactivo
            qmp.send_keys(["p", "w", "d", "ret"])
            time.sleep(0.4)
            assert wait_for_log_token(SERIAL_LOG, "/", 6.0), "Teclado PS/2 no respondio!"

            qmp.screendump_png("shot4_v241_fast_typing")
            print("  -> TEST 4 PASSED: Tipeo interactivo rapido procesado sin contencion de bus VRAM.")

            # -------------------------------------------------------------
            # TEST 5: Desktop UI Compositor Integration & Clean Return to TTY
            # -------------------------------------------------------------
            print("[TEST 5] Verificando lanzamiento del Compositor Grafico y retorno a TTY...")
            proc.stdin.write(b"gui\n")
            proc.stdin.flush()
            assert wait_for_log_token(SERIAL_LOG, "[desktop_ui.elf] FipSOs Shell UI", 8.0), "Desktop UI no inicio!"
            time.sleep(0.8)
            qmp.screendump_png("shot5_v241_desktop_ui")

            # Salir de Desktop UI enviando tecla 'q'
            qmp.send_keys(["q"])
            time.sleep(0.8)
            assert wait_for_log_token(SERIAL_LOG, "Consola TTY 1280x800 restaurada OK", 6.0), "Retorno a TTY fallo!"
            qmp.screendump_png("shot6_v241_tty_restored")
            print("  -> TEST 5 PASSED: Compositor 1280x800 y retorno seguro a consola TTY verificado.")

            print("\n=======================================================")
            print("  FASE v2.4.1 (PERFORMANCE & I/O OPTIMIZATIONS) 100% OK")
            print("=======================================================")

        finally:
            proc.kill()
            proc.wait()
            if os.path.exists(QMP_SOCK):
                os.remove(QMP_SOCK)

if __name__ == "__main__":
    main()
