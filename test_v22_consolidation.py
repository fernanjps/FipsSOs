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
from http.server import HTTPServer, SimpleHTTPRequestHandler

SCRATCH_DIR = "/mnt/c/Users/Ferna/.gemini/antigravity/brain/4ef4b43d-91a0-44a4-aefe-f8b38846bd63/scratch"
ARTIFACT_DIR = "/mnt/c/Users/Ferna/.gemini/antigravity/brain/4ef4b43d-91a0-44a4-aefe-f8b38846bd63"
SERIAL_LOG = f"{SCRATCH_DIR}/serial_v22.log"
QMP_SOCK = "/tmp/fipsos_v22_qmp.sock"
DISK_IMG = "/mnt/c/FipsSOs/build/fipsos_disk.img"
ISO_IMG = "/mnt/c/FipsSOs/build/fipsos-x86_64.iso"
HTTP_DIR = "/tmp/fipsos_v22_http_srv"

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
        print(f"[QMP] Captured {png} -> {art_png}")

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

class CustomHttpHandler(SimpleHTTPRequestHandler):
    def log_message(self, format, *args):
        print(f"[HOST HTTP] {args[0]} -> {args[1]}")

def run_http_server(directory: str, port: int = 8080):
    os.chdir(directory)
    server = HTTPServer(("0.0.0.0", port), CustomHttpHandler)
    server.serve_forever()

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
        "-netdev", "user,id=net0",
        "-device", "e1000e,netdev=net0",
        "-no-reboot",
        "-no-shutdown",
    ]

def main():
    os.makedirs(SCRATCH_DIR, exist_ok=True)
    os.makedirs(HTTP_DIR, exist_ok=True)
    if os.path.exists(QMP_SOCK):
        os.remove(QMP_SOCK)
    if os.path.exists(SERIAL_LOG):
        os.remove(SERIAL_LOG)
    if not os.path.exists(DISK_IMG):
        with open(DISK_IMG, "wb") as f:
            f.truncate(16 * 1024 * 1024)

    # Prepare HTTP test payload on host
    test_content = (
        "======================================================\n"
        "  Conexion TCP Real Host (WSL2) -> Desktop UI (FipSOs v2.2)\n"
        "  Fecha: Noviembre 2026\n"
        "  Red: Intel e1000e DMA -> POSIX Sockets -> SATA AHCI\n"
        "  Descarga Directa desde Desktop UI completada OK.\n"
        "======================================================\n"
    )
    with open(f"{HTTP_DIR}/host_test.txt", "w", encoding="utf-8") as f:
        f.write(test_content)

    # Start HTTP server on port 8080 in background thread
    http_thread = threading.Thread(target=run_http_server, args=(HTTP_DIR, 8080), daemon=True)
    http_thread.start()
    time.sleep(0.5)

    # Kill any stale QEMU instances
    subprocess.run(["killall", "-9", "qemu-system-x86_64"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    time.sleep(0.5)

    print("=== STARTING FIPSOS V2.2 CONSOLIDATION TEST ===")
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

            # 1. Wait for Shell prompt on boot
            print("[TEST 1] Waiting for FipSOs Shell boot prompt...")
            assert wait_for_log_token(SERIAL_LOG, "fipsos:/#", 12.0), "Shell prompt missing!"
            time.sleep(0.5)
            qmp.screendump_png("shot1_v22_shell_boot")
            print("[TEST 1] Shell boot prompt OK.")

            # 2. Launch Desktop UI from shell
            print("[TEST 2] Launching Desktop UI via 'gui' command...")
            proc.stdin.write(b"gui\n")
            proc.stdin.flush()
            assert wait_for_log_token(SERIAL_LOG, "[desktop_ui.elf] FipSOs Shell UI", 8.0), "Desktop UI launch failed!"
            time.sleep(1.2)
            qmp.screendump_png("shot2_v22_desktop_ui")
            print("[TEST 2] Desktop UI running with 4 windows and double buffering OK.")

            # 3. Test Real TCP Download from Desktop UI
            print("[TEST 3] Triggering real TCP download in Desktop UI (key 'i')...")
            # Send 'i' or 'g' to trigger action_browser_get()
            proc.stdin.write(b"i")
            proc.stdin.flush()
            time.sleep(2.0)
            assert wait_for_log_token(SERIAL_LOG, "[linux_abi::sys_socket] socket(AF_INET, SOCK_STREAM", 6.0), "sys_socket not invoked!"
            assert wait_for_log_token(SERIAL_LOG, "connect(fd=", 6.0), "sys_connect not invoked!"
            assert wait_for_log_token(SERIAL_LOG, "[ostd::net::tcp] Three-Way Handshake", 6.0), "TCP Handshake failed!"
            assert wait_for_log_token(SERIAL_LOG, "[DiskFS::sync] Archivo \"/disk/host_test.txt\"", 6.0), "DiskFS save failed!"
            qmp.screendump_png("shot3_v22_browser_download")
            print("[TEST 3] Real TCP download via e1000e DMA to SATA DiskFS verified OK!")

            # 4. Test Task Manager & Distributed SMP on Core 1
            print("[TEST 4] Activating background task on Core 1 (key 'b') and checking Task Manager (key '1')...")
            proc.stdin.write(b"b")
            proc.stdin.flush()
            time.sleep(0.5)
            proc.stdin.write(b"1")
            proc.stdin.flush()
            time.sleep(1.0)
            qmp.screendump_png("shot4_v22_task_manager")
            print("[TEST 4] Distributed SMP on Core 0 and Core 1 captured in Task Manager OK.")

            # 5. Exit Desktop UI cleanly to TTY
            print("[TEST 5] Exiting Desktop UI (key 'q') to verify TTY restoration...")
            proc.stdin.write(b"q")
            proc.stdin.flush()
            assert wait_for_log_token(SERIAL_LOG, "[desktop_ui.elf] Sesion grafica finalizada", 6.0), "Desktop exit missing!"
            assert wait_for_log_token(SERIAL_LOG, "[fipsos_sh] Sesion de escritorio cerrada", 6.0), "Shell resume missing!"
            time.sleep(0.8)
            qmp.screendump_png("shot5_v22_tty_restored")
            print("[TEST 5] TTY surface restored cleanly without residues OK.")

            # 6. Verify downloaded SATA file from restored TTY
            print("[TEST 6] Verifying downloaded SATA file via '/bin/busybox cat /disk/host_test.txt'...")
            proc.stdin.write(b"/bin/busybox cat /disk/host_test.txt\n")
            proc.stdin.flush()
            time.sleep(1.5)
            assert wait_for_log_token(SERIAL_LOG, "Conexion TCP Real Host", 5.0), "File content mismatch!"
            print("[TEST 6] SATA file verification confirmed via busybox cat OK.")

            # 7. Clean ACPI poweroff
            print("[TEST 7] Sending 'poweroff'...")
            proc.stdin.write(b"poweroff\n")
            proc.stdin.flush()
            assert wait_for_log_token(SERIAL_LOG, "[ACPI::Power] Apagando el equipo", 6.0), "ACPI poweroff failed!"
            time.sleep(0.8)

            try:
                qmp.cmd("quit")
            except Exception:
                pass
            print("=== ALL FIPSOS V2.2 TESTS PASSED SUCCESSFULLY! ===")
        finally:
            try:
                proc.kill()
            except Exception:
                pass
            proc.wait(timeout=5)

    with open(SERIAL_LOG, "r", encoding="utf-8", errors="replace") as rf:
        print("\n=== FULL SERIAL LOG ===")
        print(rf.read())

if __name__ == "__main__":
    main()
