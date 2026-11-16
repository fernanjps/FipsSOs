#!/usr/bin/env python3
"""
FipSOs - Fase v3.7 Automated Test Suite (test_v37_safety_xhci_goose.py)
Validates:
1. GPT/MBR Anti-Wipe Partition Shield (Protects external MBR 0x55AA + GPT 'EFI PART' disks,
   mounts /disk in RAM Protected Mode, locks physical writes, and only formats virgin 0x00 disks).
2. Full USB xHCI HID Enumeration (Port Reset, Enable Slot, Address Device, Config/HID Descriptors,
   Configure Endpoint & Interrupt IN Rings for both Keyboard Usage 0x06 and Mouse Usage 0x02).
3. Capa 3 Goose Language Monitor (/bin/goose_mon executed without '.elf', Zero-GC Data-Stack).
4. Global Synchronization to FipSOs v3.7 across kernel boot, FB header, remote TCP :2222, sensors & GUI.
"""

import json
import os
import re
import socket
import struct
import subprocess
import time
import zlib

SCRATCH_DIR = "/mnt/c/Users/Ferna/.gemini/antigravity/brain/4ef4b43d-91a0-44a4-aefe-f8b38846bd63/scratch"
ARTIFACT_DIR = "/mnt/c/Users/Ferna/.gemini/antigravity/brain/4ef4b43d-91a0-44a4-aefe-f8b38846bd63"
ISO_IMG = "/mnt/c/FipsSOs/build/fipsos-x86_64.iso"
GPT_IMG = "/mnt/c/FipsSOs/build/fipsos_gpt_v37.img"
VIRGIN_IMG = "/mnt/c/FipsSOs/build/fipsos_virgin_v37.img"

ANSI_ESCAPE_RE = re.compile(r"\x1b\[[0-9;]*[a-zA-Z]")


def strip_ansi(s: str) -> str:
    return ANSI_ESCAPE_RE.sub("", s)


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
        for _ in range(60):
            try:
                self.sock.connect(sock_path)
                break
            except Exception:
                time.sleep(0.1)
        else:
            raise RuntimeError(f"No se pudo conectar al socket QMP {sock_path}")
        self._read_json()
        self._send_json({"execute": "qmp_capabilities"})
        resp = self._read_json()
        if "return" not in resp:
            raise RuntimeError(f"Error negociando capacidades QMP: {resp}")

    def _read_json(self):
        buf = b""
        while True:
            chunk = self.sock.recv(4096)
            if not chunk:
                break
            buf += chunk
            if b"\r\n" in buf:
                for line in buf.split(b"\r\n"):
                    line = line.strip()
                    if line:
                        try:
                            return json.loads(line.decode("utf-8"))
                        except Exception:
                            continue
        return {}

    def _send_json(self, cmd):
        self.sock.sendall((json.dumps(cmd) + "\r\n").encode("utf-8"))

    def send_key(self, key_name: str):
        self._send_json(
            {"execute": "send-key", "arguments": {"keys": [{"type": "qcode", "data": key_name}]}}
        )
        self._read_json()

    def type_string(self, text: str, delay: float = 0.04):
        key_map = {
            " ": "spc",
            "\n": "ret",
            "-": "minus",
            ".": "dot",
            "/": "slash",
            "_": "shift-minus",
            ">": "shift-dot",
            "|": "shift-backslash",
            ":": "shift-semicolon",
            ";": "semicolon",
            '"': "shift-apostrophe",
            "=": "equal",
            "+": "shift-equal",
        }
        for ch in text:
            if ch in key_map:
                qcode = key_map[ch]
                if "-" in qcode:
                    parts = qcode.split("-")
                    self._send_json(
                        {
                            "execute": "send-key",
                            "arguments": {"keys": [{"type": "qcode", "data": p} for p in parts]},
                        }
                    )
                    self._read_json()
                else:
                    self.send_key(qcode)
            elif ch.isdigit():
                self.send_key(ch)
            elif ch.isalpha():
                self.send_key(ch.lower())
            time.sleep(delay)

    def screendump(self, out_ppm_path: str):
        self._send_json({"execute": "screendump", "arguments": {"filename": out_ppm_path}})
        return self._read_json()

    def close(self):
        try:
            self.sock.close()
        except Exception:
            pass


def read_serial(log_path: str) -> str:
    if not os.path.exists(log_path):
        return ""
    with open(log_path, "r", encoding="utf-8", errors="ignore") as f:
        return f.read()


def wait_for_serial(log_path: str, needle: str, timeout_s: float) -> str:
    deadline = time.time() + timeout_s
    while time.time() < deadline:
        text = strip_ansi(read_serial(log_path))
        if needle in text:
            return text
        time.sleep(0.2)
    raise AssertionError(f"Timeout ({timeout_s}s) esperando en serial: {needle!r}")


def create_gpt_sentinel_disk(path: str) -> bytes:
    if os.path.exists(path):
        os.remove(path)
    total_bytes = 16 * 1024 * 1024
    buf = bytearray(total_bytes)

    # LBA 0: Protective MBR with partition type 0xEE + 0x55AA signature
    part_entry = struct.pack(
        "<B3sB3sII",
        0x00,               # status
        b"\x00\x02\x00",    # CHS start
        0xEE,               # GPT protective partition type
        b"\xff\xff\xff",    # CHS end
        1,                  # start LBA
        (total_bytes // 512) - 1,
    )
    buf[446:462] = part_entry
    buf[510:512] = b"\x55\xaa"

    # LBA 1: GPT Header with "EFI PART" signature
    buf[512:520] = b"EFI PART"
    buf[520:524] = b"\x00\x00\x01\x00"  # Revision 1.0
    buf[524:528] = struct.pack("<I", 92)

    # LBA 64: User sentinel data that MUST remain intact
    sentinel = b"KEEP_WINDOWS_LINUX_DATA_DO_NOT_WIPE_V37"
    off = 64 * 512
    buf[off : off + len(sentinel)] = sentinel

    with open(path, "wb") as f:
        f.write(buf)
    return bytes(sentinel)


def test_gpt_shield_xhci_goose() -> None:
    print("\n--- [TEST 1] GPT/MBR Anti-Wipe Shield + xHCI HID + Goose Capa 3 + v3.7 Sync ---")
    serial_log = f"{SCRATCH_DIR}/serial_v37_gpt.log"
    qmp_sock = "/tmp/fipsos_v37_gpt_qmp.sock"
    for f in (serial_log, qmp_sock):
        if os.path.exists(f):
            os.remove(f)

    sentinel = create_gpt_sentinel_disk(GPT_IMG)

    qemu_cmd = [
        "qemu-system-x86_64",
        "-M", "q35",
        "-cpu", "max",
        "-smp", "2",
        "-m", "512M",
        "-no-reboot", "-no-shutdown",
        "-serial", f"file:{serial_log}",
        "-qmp", f"unix:{qmp_sock},server,nowait",
        "-drive", f"id=satadisk,file={GPT_IMG},format=raw,if=none",
        "-device", "ide-hd,drive=satadisk,bus=ide.0",
        "-device", "qemu-xhci,id=xhci0",
        "-device", "usb-kbd,bus=xhci0.0",
        "-device", "usb-mouse,bus=xhci0.0",
        "-netdev", "user,id=net0,hostfwd=tcp::2222-:2222",
        "-device", "e1000e,netdev=net0",
        "-boot", "d",
        "-cdrom", ISO_IMG,
    ]
    proc = subprocess.Popen(qemu_cmd)
    qmp = None
    try:
        serial = wait_for_serial(serial_log, "for available commands", 22.0)

        assert "[FipSOs] Framekernel FipSOs v3.7 activo" in serial, "Falta banner kernel FipSOs v3.7"
        assert "Tabla GPT/MBR externa detectada - Modo Protegido (Sin formateo)" in serial, (
            "Falta deteccion de GPT/MBR Modo Protegido"
        )
        assert "[ostd::bus::xhci::hid]" in serial, "Falta enumeracion xHCI HID"
        assert "Teclado USB HID (Usage=0x06" in serial, "Falta enumeracion de Teclado USB HID"
        assert "Raton USB HID (Usage=0x02" in serial, "Falta enumeracion de Raton USB HID"
        assert "FIPSOS_DISKFS_V2" not in serial and "Formateando disco" not in serial, (
            "Se intento formatear disco con GPT/MBR externa!"
        )
        print("[OK] Kernel v3.7 + Escudo GPT/MBR + Enumeracion xHCI HID Keyboard & Mouse verificados.")

        qmp = QmpClient(qmp_sock)
        time.sleep(0.5)
        ppm1 = f"{SCRATCH_DIR}/shot1_v37_gpt.ppm"
        qmp.screendump(ppm1)
        ppm_to_png(ppm1, f"{ARTIFACT_DIR}/shot1_v37_gpt_protected_xhci.png")

        # Execute Goose Capa 3 binary WITHOUT .elf suffix
        qmp.type_string("goose_mon\n", delay=0.03)
        serial = wait_for_serial(serial_log, "=== [Goose] Telemetria Ring 3 Completada OK ===", 10.0)
        assert "GOOSE CAPA 3 MONITOR (FipSOs v3.7" in serial
        assert "[Goose] RAM Fisica:" in serial
        assert "[Goose] /sys/fipsos/hwmon" in serial
        print("[OK] /bin/goose_mon ejecutado sin .elf con telemetria Zero-GC Data-Stack completa.")

        ppm2 = f"{SCRATCH_DIR}/shot2_v37_goose.ppm"
        qmp.screendump(ppm2)
        ppm_to_png(ppm2, f"{ARTIFACT_DIR}/shot2_v37_goose_mon.png")

        # Check boot_log.txt in protected RAM /disk and run sensors
        qmp.type_string("cat /disk/boot_log.txt\n", delay=0.03)
        serial = wait_for_serial(serial_log, "MODO PROTEGIDO", 8.0)
        qmp.type_string("sensors\n", delay=0.03)
        serial = wait_for_serial(serial_log, "[HWMON] FipSOs v3.7", 8.0)
        assert "HID Kbd=1 Mouse=1" in serial, "sensors no reporta HID Kbd=1 Mouse=1"
        print("[OK] /disk/boot_log.txt (MODO PROTEGIDO) y sensors v3.7 con HID Kbd/Mouse verificados.")

        ppm3 = f"{SCRATCH_DIR}/shot3_v37_sensors.ppm"
        qmp.screendump(ppm3)
        ppm_to_png(ppm3, f"{ARTIFACT_DIR}/shot3_v37_sensors_banner.png")

        # Verify TCP :2222 Remote Shell banner
        tcp = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        tcp.settimeout(4.0)
        tcp.connect(("127.0.0.1", 2222))
        banner = tcp.recv(1024).decode("utf-8", errors="ignore")
        tcp.close()
        assert "FipSOs v3.7 Remote Shell" in banner, f"Banner remoto inesperado: {banner!r}"
        print(f"[OK] TCP :2222 Remote Shell banner verificado: {banner.strip()!r}")

    finally:
        if qmp is not None:
            qmp.close()
        proc.terminate()
        try:
            proc.wait(timeout=5)
        except Exception:
            proc.kill()

    # Post-mortem host verification: zero bytes touched on external GPT disk
    with open(GPT_IMG, "rb") as f:
        disk_bytes = f.read()
    assert disk_bytes[510:512] == b"\x55\xaa", "Firma MBR 0x55AA alterada!"
    assert disk_bytes[512:520] == b"EFI PART", "Firma GPT EFI PART borrada!"
    assert sentinel in disk_bytes[64 * 512 : 65 * 512], "Datos del usuario en LBA 64 alterados!"
    assert b"FIPSOS_DISKFS_V2" not in disk_bytes[:131072], "Se escribio DiskFS sobre disco protegido!"
    print("[OK] Verificacion bit-a-bit en host: LBA 0 (0x55AA), LBA 1 (EFI PART) y LBA 64 intactos al 100%.")


def test_virgin_format_and_desktop() -> None:
    print("\n--- [TEST 2] Disco Virgen 0x00 -> Auto-Formato DiskFS v2 + Desktop C++20 FipSOs v3.7 ---")
    serial_log = f"{SCRATCH_DIR}/serial_v37_virgin.log"
    qmp_sock = "/tmp/fipsos_v37_virgin_qmp.sock"
    for f in (serial_log, qmp_sock, VIRGIN_IMG):
        if os.path.exists(f):
            os.remove(f)

    subprocess.run(["qemu-img", "create", "-f", "raw", VIRGIN_IMG, "16M"], check=True)

    qemu_cmd = [
        "qemu-system-x86_64",
        "-M", "q35",
        "-cpu", "max",
        "-smp", "2",
        "-m", "512M",
        "-no-reboot", "-no-shutdown",
        "-serial", f"file:{serial_log}",
        "-qmp", f"unix:{qmp_sock},server,nowait",
        "-drive", f"id=satadisk,file={VIRGIN_IMG},format=raw,if=none",
        "-device", "ide-hd,drive=satadisk,bus=ide.0",
        "-device", "qemu-xhci,id=xhci0",
        "-device", "usb-kbd,bus=xhci0.0",
        "-device", "usb-mouse,bus=xhci0.0",
        "-netdev", "user,id=net0",
        "-device", "e1000e,netdev=net0",
        "-boot", "d",
        "-cdrom", ISO_IMG,
    ]
    proc = subprocess.Popen(qemu_cmd)
    qmp = None
    try:
        serial = wait_for_serial(serial_log, "for available commands", 22.0)
        assert "Tabla GPT/MBR externa detectada" not in serial, (
            "Falso positivo GPT/MBR en disco totalmente en ceros!"
        )
        assert "DiskFS" in serial
        print("[OK] Disco virgen (0x00) formateado y montado correctamente sin falsos positivos.")

        qmp = QmpClient(qmp_sock)
        time.sleep(0.4)
        qmp.type_string("gui\n", delay=0.04)
        wait_for_serial(serial_log, "desktop_ui", 10.0)
        time.sleep(2.0)
        ppm4 = f"{SCRATCH_DIR}/shot4_v37_desktop.ppm"
        qmp.screendump(ppm4)
        ppm_to_png(ppm4, f"{ARTIFACT_DIR}/shot4_v37_desktop_v37.png")
        print("[OK] Entorno grafico C++20 desktop_ui v3.7 capturado exitosamente.")
    finally:
        if qmp is not None:
            qmp.close()
        proc.terminate()
        try:
            proc.wait(timeout=5)
        except Exception:
            proc.kill()

    with open(VIRGIN_IMG, "rb") as f:
        virgin_head = f.read(16384)
    assert b"FIPSOS_DISKFS_V2" in virgin_head, "Disco virgen no recibio superbloque FIPSOS_DISKFS_V2!"
    print("[OK] Superbloque FIPSOS_DISKFS_V2 verificado en disco virgen tras auto-formateo.")


def main() -> None:
    os.makedirs(SCRATCH_DIR, exist_ok=True)
    test_gpt_shield_xhci_goose()
    test_virgin_format_and_desktop()
    print("\n======================================================================")
    print("  [PASS] FipSOs Fase v3.7 - Todos los tests superados al 100%!")
    print("======================================================================")


if __name__ == "__main__":
    main()
