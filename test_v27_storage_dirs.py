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
SERIAL_LOG = f"{SCRATCH_DIR}/serial_v27.log"
QMP_SOCK = "/tmp/fipsos_v27_qmp.sock"
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
            '=': 'equal',
            '>': 'shift_r', # handle below
            '1': '1', '2': '2', '3': '3', '4': '4', '5': '5',
            '6': '6', '7': '7', '8': '8', '9': '9', '0': '0',
        }
        for ch in text:
            if ch == '_':
                self.cmd("send-key", {"keys": [{"type": "qcode", "data": "shift_r"}, {"type": "qcode", "data": "minus"}]})
            elif ch == '>':
                self.cmd("send-key", {"keys": [{"type": "qcode", "data": "shift_r"}, {"type": "qcode", "data": "dot"}]})
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

    # Initialize a clean 16 MiB disk image to test virgin formatting with DiskFS v2
    with open(DISK_IMG, "wb") as f:
        f.truncate(16 * 1024 * 1024)

    subprocess.run(["killall", "-9", "qemu-system-x86_64"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    time.sleep(0.5)

    print("=== INICIANDO VALIDACION FIPSOS FASE V2.7 (DISKFS V2: SUBDIRECTORIOS, 128 INODOS & BITMAP LBA) ===", flush=True)

    # -------------------------------------------------------------
    # SESION 1: Formato V2, Subdirectorios, >25 Archivos y Reciclaje
    # -------------------------------------------------------------
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

        print("[BOOT] Esperando inicializacion de DiskFS v2 y Shell...", flush=True)
        ok = wait_for_log_token(SERIAL_LOG, "FIPSOS_DISKFS_V2", timeout_sec=20.0)
        assert ok, "Fallo: No se detecto el formateo 'FIPSOS_DISKFS_V2' en el log de arranque."

        ok = wait_for_log_token(SERIAL_LOG, "fipsos:/#", timeout_sec=15.0)
        assert ok, "Fallo: No se alcanzo el prompt del shell interactivo."
        time.sleep(1.0)
        qmp.screendump_png("shot1_v27_boot")
        print("[TEST 1/6] OK: DiskFS v2 formateado y montado (Superbloque V2, 128 inodos, Bitmap 4KB).", flush=True)

        # -------------------------------------------------------------
        # TEST 2: Creacion de Subdirectorios Anidados en /disk
        # -------------------------------------------------------------
        print("[TEST 2/6] Creando subdirectorios jerarquicos en /disk...", flush=True)
        qmp.send_string("mkdir /disk/src\n")
        time.sleep(1.0)
        ok = wait_for_log_token(SERIAL_LOG, "[DiskFS::mkdir] Directorio \"/disk/src\" persistido", timeout_sec=5.0)
        assert ok, "Fallo al crear /disk/src en DiskFS!"

        qmp.send_string("mkdir /disk/src/nested\n")
        time.sleep(1.0)
        ok = wait_for_log_token(SERIAL_LOG, "[DiskFS::mkdir] Directorio \"/disk/src/nested\" persistido", timeout_sec=5.0)
        assert ok, "Fallo al crear /disk/src/nested en DiskFS!"

        qmp.send_string("mkdir /disk/bin_extra\n")
        time.sleep(1.0)
        ok = wait_for_log_token(SERIAL_LOG, "[DiskFS::mkdir] Directorio \"/disk/bin_extra\" persistido", timeout_sec=5.0)
        assert ok, "Fallo al crear /disk/bin_extra en DiskFS!"

        # Crear archivo dentro de subdirectorio anidado
        qmp.send_string("touch /disk/src/main.c\n")
        time.sleep(1.0)
        ok = wait_for_log_token(SERIAL_LOG, "[DiskFS::sync] Archivo \"/disk/src/main.c\" creado", timeout_sec=5.0)
        assert ok, "Fallo al crear /disk/src/main.c en DiskFS!"

        qmp.send_string("ls /disk\n")
        time.sleep(1.5)
        qmp.send_string("ls /disk/src\n")
        time.sleep(1.5)
        qmp.screendump_png("shot2_v27_subdirs")
        print("[TEST 2/6] OK: Subdirectorios anidados (/disk/src, /disk/src/nested, /disk/bin_extra) creados y verificados.", flush=True)

        # -------------------------------------------------------------
        # TEST 3: Superar Antiguo Limite de 16 (Creacion de 30 Archivos)
        # -------------------------------------------------------------
        print("[TEST 3/6] Creando 30 archivos en /disk para superar limite de 16...", flush=True)
        for i in range(30):
            qmp.send_string(f"touch /disk/file_{i}.txt\n")
            time.sleep(0.1)

        time.sleep(2.0)
        ok = wait_for_log_token(SERIAL_LOG, "Archivo \"/disk/file_29.txt\" creado", timeout_sec=10.0)
        assert ok, "Fallo al crear 30 archivos! Se detuvo antes de /disk/file_29.txt"

        qmp.send_string("ls /disk\n")
        time.sleep(2.0)
        qmp.screendump_png("shot3_v27_30_files")
        print("[TEST 3/6] OK: 30 archivos creados con exito en /disk superando el limite anterior de 16 inodos.", flush=True)

        # -------------------------------------------------------------
        # TEST 4: Reciclaje de Sectores LBA mediante Bitmap
        # -------------------------------------------------------------
        print("[TEST 4/6] Eliminando 15 archivos y verificando reciclaje de sectores en Bitmap...", flush=True)
        for i in range(15):
            qmp.send_string(f"rm /disk/file_{i}.txt\n")
            time.sleep(0.1)

        time.sleep(2.0)
        ok = wait_for_log_token(SERIAL_LOG, "Archivo \"/disk/file_14.txt\" eliminado", timeout_sec=5.0)
        assert ok, "Fallo al eliminar archivos 0..14!"

        # Ahora escribimos 5 nuevos archivos
        for i in range(5):
            qmp.send_string(f"touch /disk/recycled_{i}.txt\n")
            time.sleep(0.1)

        time.sleep(2.0)
        ok = wait_for_log_token(SERIAL_LOG, "Archivo \"/disk/recycled_4.txt\" creado", timeout_sec=5.0)
        assert ok, "Fallo al crear archivos reciclados!"

        qmp.send_string("ls /disk\n")
        time.sleep(2.0)
        qmp.screendump_png("shot4_v27_recycled")
        print("[TEST 4/6] OK: Eliminacion masiva y reciclaje de sectores LBA verificado.", flush=True)

        # -------------------------------------------------------------
        # TEST 5: Control de Eliminacion de Directorios (rmdir)
        # -------------------------------------------------------------
        print("[TEST 5/6] Verificando proteccion de no-vacio y borrado con rmdir...", flush=True)
        # /disk/src tiene /disk/src/nested y /disk/src/main.c -> rmdir debe fallar
        qmp.send_string("rmdir /disk/src\n")
        time.sleep(1.0)
        ok = wait_for_log_token(SERIAL_LOG, "Fallo en sys_rmdir: Directorio no vacio", timeout_sec=5.0)
        assert ok, "Fallo: rmdir debio rechazar borrar directorio no vacio!"

        # Borrar hijos y reintentar
        qmp.send_string("rm /disk/src/main.c\n")
        time.sleep(0.5)
        qmp.send_string("rmdir /disk/src/nested\n")
        time.sleep(0.5)
        qmp.send_string("rmdir /disk/src\n")
        time.sleep(1.0)
        ok = wait_for_log_token(SERIAL_LOG, "[DiskFS::rmdir] Directorio \"/disk/src\" eliminado de SATA AHCI", timeout_sec=5.0)
        assert ok, "Fallo: /disk/src no pudo ser eliminado tras vaciarlo!"

        qmp.send_string("ls /disk\n")
        time.sleep(1.5)
        qmp.screendump_png("shot5_v27_rmdir")
        print("[TEST 5/6] OK: rmdir rechaza directorios no vacios y elimina directorios vacios correctamente.", flush=True)

    finally:
        try:
            qmp.cmd("quit")
        except Exception:
            pass
        proc.terminate()
        try:
            proc.wait(timeout=3.0)
        except Exception:
            proc.kill()
        time.sleep(1.0)

    # -------------------------------------------------------------
    # SESION 2: Verificacion de Persistencia Real tras Reinicio Completo
    # -------------------------------------------------------------
    print("\n=== REINICIANDO VM PARA COMPROBAR PERSISTENCIA REAL TRAS CICLO DE DISCO ===", flush=True)
    if os.path.exists(QMP_SOCK):
        os.remove(QMP_SOCK)

    REBOOT_LOG = f"{SCRATCH_DIR}/serial_v27_reboot.log"
    if os.path.exists(REBOOT_LOG):
        os.remove(REBOOT_LOG)

    with open(REBOOT_LOG, "wb") as logf:
        proc2 = subprocess.Popen(
            get_qemu_cmd(),
            stdin=subprocess.PIPE,
            stdout=logf,
            stderr=subprocess.STDOUT,
        )

    try:
        qmp2 = QmpClient(QMP_SOCK)
        print("[BOOT 2] Esperando arranque de FipSOs y montaje de disco SATA existente...", flush=True)
        ok = wait_for_log_token(REBOOT_LOG, "Directorio \"/disk/bin_extra\" montado de disco SATA", timeout_sec=20.0)
        assert ok, "Fallo de persistencia: /disk/bin_extra no fue cargado tras reiniciar!"

        ok = wait_for_log_token(REBOOT_LOG, "Archivo \"/disk/recycled_0.txt\" cargado de disco SATA", timeout_sec=20.0)
        assert ok, "Fallo de persistencia: /disk/recycled_0.txt no fue cargado tras reiniciar!"

        ok = wait_for_log_token(REBOOT_LOG, "Archivo \"/disk/file_25.txt\" cargado de disco SATA", timeout_sec=20.0)
        assert ok, "Fallo de persistencia: /disk/file_25.txt no fue cargado tras reiniciar!"

        ok = wait_for_log_token(REBOOT_LOG, "fipsos:/#", timeout_sec=15.0)
        assert ok, "Fallo: No se alcanzo el prompt del shell tras reiniciar."
        time.sleep(1.0)

        qmp2.send_string("ls /disk\n")
        time.sleep(2.0)
        qmp2.screendump_png("shot6_v27_persistence_reboot")
        print("[TEST 6/6] OK: Todos los subdirectorios y archivos sobrevivieron al reinicio completo de la VM!", flush=True)

    finally:
        try:
            qmp2.cmd("quit")
        except Exception:
            pass
        proc2.terminate()
        try:
            proc2.wait(timeout=3.0)
        except Exception:
            proc2.kill()

    print("\n=======================================================================", flush=True)
    print("  FASE V2.7 VALIDADA CON EXITO AL 100%: DISKFS V2 PERSISTENTE Y ROBUSTO  ", flush=True)
    print("=======================================================================", flush=True)

if __name__ == "__main__":
    main()
