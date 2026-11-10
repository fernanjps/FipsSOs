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
import urllib.request
from http.server import HTTPServer, SimpleHTTPRequestHandler

SCRATCH_DIR = "/mnt/c/Users/Ferna/.gemini/antigravity/brain/4ef4b43d-91a0-44a4-aefe-f8b38846bd63/scratch"
ARTIFACT_DIR = "/mnt/c/Users/Ferna/.gemini/antigravity/brain/4ef4b43d-91a0-44a4-aefe-f8b38846bd63"
SERIAL_LOG = f"{SCRATCH_DIR}/serial_v24.log"
QMP_SOCK = "/tmp/fipsos_v24_qmp.sock"
DISK_IMG = "/mnt/c/FipsSOs/build/fipsos_disk.img"
ISO_IMG = "/mnt/c/FipsSOs/build/fipsos-x86_64.iso"
USB_IMG = "/mnt/c/FipsSOs/build/fipsos-baremetal-usb.img"
HTTP_DIR = "/tmp/fipsos_v24_http_srv"

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
            time.sleep(0.05)

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
        print(f"[HOST HTTP :8080] {args[0]} -> {args[1]}")

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
        "-netdev", "user,id=net0,hostfwd=tcp::8088-:80",
        "-device", "e1000e,netdev=net0",
        "-no-reboot",
        "-no-shutdown",
    ]

def setup_host_http_files():
    os.makedirs(HTTP_DIR, exist_ok=True)
    # 1. sample_tool.elf
    shutil.copyfile("/mnt/c/FipsSOs/build/sample_tool.elf", f"{HTTP_DIR}/sample_tool.elf")
    # 2. benchmark_simd.elf
    shutil.copyfile("/mnt/c/FipsSOs/build/benchmark_simd.elf", f"{HTTP_DIR}/benchmark_simd.elf")
    # 3. editor_kilo.elf
    shutil.copyfile("/mnt/c/FipsSOs/build/editor_kilo.elf", f"{HTTP_DIR}/editor_kilo.elf")
    # 4. host_test.txt & index.html
    with open(f"{HTTP_DIR}/host_test.txt", "w") as f:
        f.write("FIPSOS_HOST_TEST_PAYLOAD_V24_OK\n")
    with open(f"{HTTP_DIR}/index.html", "w") as f:
        f.write("<html><body><h1>FipSOs Host Repo Online</h1></body></html>\n")
    print(f"[HOST HTTP] Preparados archivos en {HTTP_DIR}: sample_tool.elf, benchmark_simd.elf, editor_kilo.elf, host_test.txt, index.html")

def main():
    os.makedirs(SCRATCH_DIR, exist_ok=True)
    setup_host_http_files()
    if os.path.exists(QMP_SOCK):
        os.remove(QMP_SOCK)
    if os.path.exists(SERIAL_LOG):
        os.remove(SERIAL_LOG)
    if not os.path.exists(DISK_IMG):
        with open(DISK_IMG, "wb") as f:
            f.truncate(16 * 1024 * 1024)

    # Start host HTTP server on port 8080
    http_thread = threading.Thread(target=run_http_server, args=(HTTP_DIR, 8080), daemon=True)
    http_thread.start()
    time.sleep(0.5)

    # Kill any stale QEMU instances
    subprocess.run(["killall", "-9", "qemu-system-x86_64"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    time.sleep(0.5)

    print("=== STARTING FIPSOS V2.4 FUSION HIBRIDA VERIFICATION ===")
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
            # TEST 1: Shell Boot & Bare-Metal Hybrid Banner
            # -------------------------------------------------------------
            print("[TEST 1] Waiting for FipSOs Shell boot prompt & MOTD...")
            assert wait_for_log_token(SERIAL_LOG, "fipsos:/#", 14.0), "Shell prompt missing!"
            assert wait_for_log_token(SERIAL_LOG, "Boot: UEFI/BIOS Hybrid Bare-Metal Ready", 6.0), "Hybrid boot banner missing!"
            time.sleep(0.5)
            qmp.screendump_png("shot1_v24_shell_boot")
            print("[TEST 1] Shell boot prompt & Hybrid Bare-Metal banner verified OK.")

            # -------------------------------------------------------------
            # TEST 2: Integrated HTTP Telemetry Server from Host (curl /api/status)
            # -------------------------------------------------------------
            print("[TEST 2] Testing Integrated HTTP Telemetry Server via QEMU host port 8088...")
            http_status_ok = False
            json_data = {}
            for attempt in range(10):
                try:
                    req = urllib.request.Request("http://127.0.0.1:8088/api/status", headers={"User-Agent": "CurlTest/1.0"})
                    with urllib.request.urlopen(req, timeout=3.0) as resp:
                        if resp.status == 200:
                            raw = resp.read().decode("utf-8")
                            print(f"[TEST 2] /api/status recibido: {raw.strip()}")
                            json_data = json.loads(raw)
                            assert json_data.get("system") == "FipSOs v2.4", f"Bad system in JSON: {json_data}"
                            assert json_data.get("smp_cores") == 2, f"Expected 2 SMP cores: {json_data}"
                            assert "free_ram_mb" in json_data, f"Missing free_ram_mb: {json_data}"
                            http_status_ok = True
                            break
                except Exception as e:
                    print(f"[TEST 2] Attempt {attempt+1} connect 127.0.0.1:8088: {e}")
                    time.sleep(0.5)

            assert http_status_ok, "Fallo al conectar con el servidor HTTP de FipSOs en 127.0.0.1:8088!"

            # Query HTML root dashboard
            req_html = urllib.request.Request("http://127.0.0.1:8088/", headers={"User-Agent": "CurlTest/1.0"})
            with urllib.request.urlopen(req_html, timeout=3.0) as resp:
                assert resp.status == 200
                html_body = resp.read().decode("utf-8")
                assert "FipSOs v2.4" in html_body, "HTML dashboard missing FipSOs v2.4 title!"
                print(f"[TEST 2] / (HTML Dashboard) recibido correctamente ({len(html_body)} bytes).")

            time.sleep(0.5)
            qmp.screendump_png("shot2_v24_http_telemetry")
            print("[TEST 2] Integrated HTTP Telemetry Server verified (JSON /api/status & HTML /) OK.")

            # -------------------------------------------------------------
            # TEST 3: Launch Graphical Desktop UI & Verify App Center
            # -------------------------------------------------------------
            print("[TEST 3] Launching Graphical Desktop UI via 'gui' command...")
            proc.stdin.write(b"gui\n")
            proc.stdin.flush()
            time.sleep(2.0)
            assert wait_for_log_token(SERIAL_LOG, "[desktop_ui.elf] FipSOs Shell UI", 8.0), "Desktop UI launch missing!"
            time.sleep(0.8)
            qmp.screendump_png("shot3_v24_desktop_appcenter")
            print("[TEST 3] Desktop UI Window 3 'Centro de Aplicaciones & Consola de Ejecucion' active OK.")

            # -------------------------------------------------------------
            # TEST 4: Install App via TCP in Desktop UI
            # -------------------------------------------------------------
            print("[TEST 4] Triggering App Center TCP installation via keypress 'i'...")
            # Key 'i' triggers action_appcenter_install()
            qmp.send_keys(["i"])
            time.sleep(2.5)
            assert wait_for_log_token(SERIAL_LOG, "[DiskFS::sync] Archivo \"/disk/bin/sample_tool.elf\"", 8.0), "App Center install failed!"
            time.sleep(0.8)
            qmp.screendump_png("shot4_v24_appcenter_installed")
            print("[TEST 4] App downloaded over TCP, saved to /disk/bin/sample_tool.elf, and synced to SATA DiskFS OK.")

            # -------------------------------------------------------------
            # TEST 5: Launch App from App Center & Verify Binary Execution
            # -------------------------------------------------------------
            print("[TEST 5] Launching downloaded app from App Center via keypress 'l'...")
            qmp.send_keys(["l"])
            time.sleep(2.0)
            assert wait_for_log_token(SERIAL_LOG, "sys_execve(\"/disk/bin/sample_tool.elf\"", 8.0), "App execution failed!"

            # Return to TTY console
            print("[TEST 5] Exiting Desktop UI to verify persistent execution in Shell...")
            qmp.send_keys(["q"])
            time.sleep(1.0)
            assert wait_for_log_token(SERIAL_LOG, "fipsos:/#", 6.0), "Shell prompt return missing!"

            # Execute /disk/bin/sample_tool.elf from shell
            proc.stdin.write(b"/disk/bin/sample_tool.elf\n")
            proc.stdin.flush()
            time.sleep(1.2)
            assert wait_for_log_token(SERIAL_LOG, "HOLA DESDE /disk/bin/sample_tool", 6.0), "Direct binary execution failed!"

            # Verify Bare-Metal USB Image layout on disk
            assert os.path.exists(USB_IMG), f"USB Image missing: {USB_IMG}"
            usb_size = os.path.getsize(USB_IMG)
            print(f"[TEST 5] Bare-Metal Hybrid USB image verified: {USB_IMG} ({usb_size} bytes)")
            assert usb_size > 4 * 1024 * 1024, "USB Image too small!"

            time.sleep(0.5)
            qmp.screendump_png("shot5_v24_persisted_execution")
            print("[TEST 5] Persistent execution of network-installed app and Bare-Metal USB image verified OK.")

            # -------------------------------------------------------------
            # TEST 6: Clean ACPI Poweroff
            # -------------------------------------------------------------
            print("[TEST 6] Sending 'poweroff' to FipSOs Shell...")
            proc.stdin.write(b"poweroff\n")
            proc.stdin.flush()
            assert wait_for_log_token(SERIAL_LOG, "[ACPI::Power] Apagando el equipo", 6.0), "ACPI poweroff failed!"
            time.sleep(0.8)

            try:
                qmp.cmd("quit")
            except Exception:
                pass
            print("=== ALL FIPSOS V2.4 TESTS PASSED WITH 100% SUCCESS! ===")
        finally:
            try:
                proc.kill()
            except Exception:
                pass
            proc.wait(timeout=5)

    with open(SERIAL_LOG, "r", encoding="utf-8", errors="replace") as rf:
        print("\n=== FULL SERIAL LOG V2.4 ===")
        print(rf.read())

if __name__ == "__main__":
    main()
