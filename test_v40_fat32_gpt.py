#!/usr/bin/env python3
"""
FipSOs - Fase v4.0 Automated Test Suite (test_v40_fat32_gpt.py)
Validates:
1. GPT Partition Table Parser & /mnt/fat0 Real Read/Write FAT32 Driver:
   - Builds a compliant 64 MiB GPT + FAT32 disk image on host with LFN file `desde_host.txt`.
   - Boots FipSOs v4.0 in QEMU, verifies anti-wipe shield preserves LBA 0/GPT while
     FAT32 mounts /mnt/fat0 automatically.
   - Executes `ls /mnt/fat0`, `cat /mnt/fat0/desde_host.txt`, `mkdir /mnt/fat0/fipsos_out`,
     `echo ... > /mnt/fat0/fipsos_out/desde_fipsos.txt`, `cat /proc/mounts`, and `ps`.
   - Verifies TCP :2222 FipSOs v4.0 Remote Shell banner.
   - After QEMU shutdown, inspects raw GPT+FAT32 disk image on host: verifies GPT CRC32s
     intact, FAT1 == FAT2 bit-for-bit, valid LFN+8.3 directory entries (`.` / `..`),
     and exact `desde_fipsos.txt` file payload.
2. MBR (0x0C FAT32 LBA) Partition Parser Test:
   - Boots with an MBR-partitioned FAT32 disk and verifies `/mnt/fat0/desde_mbr.txt`.
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
GPT_FAT32_IMG = "/mnt/c/FipsSOs/build/fipsos_gpt_fat32_v40.img"
MBR_FAT32_IMG = "/mnt/c/FipsSOs/build/fipsos_mbr_fat32_v40.img"

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

    def type_string(self, text: str, delay: float = 0.035):
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
            elif ch.isupper():
                self._send_json(
                    {
                        "execute": "send-key",
                        "arguments": {
                            "keys": [
                                {"type": "qcode", "data": "shift"},
                                {"type": "qcode", "data": ch.lower()},
                            ]
                        },
                    }
                )
                self._read_json()
            elif ch.isalpha():
                self.send_key(ch.lower())
            time.sleep(delay)

    def screendump(self, out_ppm_path: str):
        self._send_json({"execute": "screendump", "arguments": {"filename": out_ppm_path}})
        return self._read_json()

    def quit(self):
        try:
            self._send_json({"execute": "quit"})
            self._read_json()
        except Exception:
            pass
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


def fat_checksum_83(raw83: bytes) -> int:
    s = 0
    for b in raw83[:11]:
        s = (((s & 1) << 7) | (s >> 1)) + b
        s &= 0xFF
    return s


def make_lfn_and_83(long_name: str, raw83: bytes, attr: int, cluster: int, size: int) -> bytearray:
    csum = fat_checksum_83(raw83)
    utf16 = [ord(c) for c in long_name]
    count = max(1, (len(utf16) + 12) // 13)
    out = bytearray()
    for rev in range(count - 1, -1, -1):
        rec = bytearray(32)
        ord_val = rev + 1
        rec[0] = (ord_val | 0x40) if ord_val == count else ord_val
        rec[11] = 0x0F
        rec[13] = csum
        base = rev * 13
        slots = [(1, 5, 0), (14, 6, 5), (28, 2, 11)]
        for start, seg_len, loc in slots:
            for j in range(seg_len):
                idx = base + loc + j
                code = utf16[idx] if idx < len(utf16) else (0x0000 if idx == len(utf16) else 0xFFFF)
                struct.pack_into("<H", rec, start + j * 2, code)
        out.extend(rec)

    srec = bytearray(32)
    srec[0:11] = raw83
    srec[11] = attr
    struct.pack_into("<H", srec, 20, (cluster >> 16) & 0xFFFF)
    struct.pack_into("<H", srec, 26, cluster & 0xFFFF)
    struct.pack_into("<I", srec, 28, size)
    out.extend(srec)
    return out


def format_fat32_partition(buf: bytearray, part_lba: int, part_sectors: int, init_name: str, init_short: bytes, init_data: bytes):
    part_off = part_lba * 512
    spc = 1  # 512B/cluster so >= 65525 clusters on ~63 MiB partition -> genuine FAT32 spec!
    reserved = 32
    num_fats = 2
    fat_size = 1008  # covers up to 129024 clusters cleanly

    # 1. Boot Sector (VBR) & Backup Boot Sector (rel LBA 6)
    vbr = bytearray(512)
    vbr[0:3] = b"\xEB\x58\x90"
    vbr[3:11] = b"FIPSOS40"
    struct.pack_into("<H", vbr, 11, 512)            # BytesPerSec
    vbr[13] = spc                                   # SecPerClus
    struct.pack_into("<H", vbr, 14, reserved)       # RsvdSecCnt
    vbr[16] = num_fats                              # NumFATs
    struct.pack_into("<H", vbr, 17, 0)              # RootEntCnt (0 for FAT32)
    struct.pack_into("<H", vbr, 19, 0)              # TotSec16 (0)
    vbr[21] = 0xF8                                  # Media
    struct.pack_into("<H", vbr, 24, 63)             # SecPerTrk
    struct.pack_into("<H", vbr, 26, 255)            # NumHeads
    struct.pack_into("<I", vbr, 28, part_lba)       # HiddSec
    struct.pack_into("<I", vbr, 32, part_sectors)   # TotSec32
    struct.pack_into("<I", vbr, 36, fat_size)       # FATSz32
    struct.pack_into("<H", vbr, 40, 0)              # ExtFlags (mirrored)
    struct.pack_into("<H", vbr, 42, 0)              # FSVer 0.0
    struct.pack_into("<I", vbr, 44, 2)              # RootClus = 2
    struct.pack_into("<H", vbr, 48, 1)              # FSInfo = 1
    struct.pack_into("<H", vbr, 50, 6)              # BkBootSec = 6
    vbr[64] = 0x80
    vbr[66] = 0x29
    struct.pack_into("<I", vbr, 67, 0x20260400)
    vbr[71:82] = b"FIPSOSFAT32"
    vbr[82:90] = b"FAT32   "
    vbr[510:512] = b"\x55\xAA"

    buf[part_off : part_off + 512] = vbr
    buf[part_off + 6 * 512 : part_off + 7 * 512] = vbr

    # 2. FSInfo at rel LBA 1
    data_sectors = part_sectors - reserved - num_fats * fat_size
    total_clusters = data_sectors // spc
    assert total_clusters >= 65525, f"Necesita >= 65525 clusters para FAT32 estricto, hay {total_clusters}"
    fsi = bytearray(512)
    struct.pack_into("<I", fsi, 0, 0x41615252)
    struct.pack_into("<I", fsi, 484, 0x61417272)
    struct.pack_into("<I", fsi, 488, total_clusters - 2)  # cluster 2 (root) & cluster 3 (init file) used
    struct.pack_into("<I", fsi, 492, 4)
    struct.pack_into("<I", fsi, 508, 0xAA550000)
    buf[part_off + 512 : part_off + 1024] = fsi
    buf[part_off + 7 * 512 : part_off + 8 * 512] = fsi

    # 3. Initialize FAT1 and FAT2 identically
    fat_sec = bytearray(512)
    struct.pack_into("<I", fat_sec, 0, 0x0FFFFFF8)
    struct.pack_into("<I", fat_sec, 4, 0x0FFFFFFF)
    struct.pack_into("<I", fat_sec, 8, 0x0FFFFFFF)   # cluster 2 = root dir EOC
    struct.pack_into("<I", fat_sec, 12, 0x0FFFFFFF)  # cluster 3 = initial host file EOC
    fat1_off = part_off + reserved * 512
    fat2_off = fat1_off + fat_size * 512
    buf[fat1_off : fat1_off + 512] = fat_sec
    buf[fat2_off : fat2_off + 512] = fat_sec

    # 4. Data region: cluster 2 = root dir with initial file entry, cluster 3 = file bytes
    data_off = fat2_off + fat_size * 512
    root_entries = make_lfn_and_83(init_name, init_short, 0x20, 3, len(init_data))
    buf[data_off : data_off + len(root_entries)] = root_entries
    clus3_off = data_off + 512  # cluster 3 = 1 sector after cluster 2
    buf[clus3_off : clus3_off + len(init_data)] = init_data


def create_gpt_fat32_disk(path: str, host_payload: bytes) -> tuple[bytes, bytes]:
    if os.path.exists(path):
        os.remove(path)
    total_sectors = 131072  # 64 MiB
    buf = bytearray(total_sectors * 512)

    part_start = 2048
    part_end = total_sectors - 34  # leaves backup GPT entries + header cleanly at end
    part_sectors = part_end - part_start + 1

    # Protective MBR (LBA 0)
    buf[446:462] = struct.pack(
        "<B3sB3sII", 0x00, b"\x00\x02\x00", 0xEE, b"\xff\xff\xff", 1, total_sectors - 1
    )
    buf[510:512] = b"\x55\xAA"

    # GPT Entries Array (128 * 128 = 16384 bytes = 32 sectors)
    entries = bytearray(128 * 128)
    # Microsoft Basic Data GUID EBD0A0A2-B9E5-4433-87C0-68B6B72699C7
    type_guid = bytes.fromhex("a2a0d0ebe5b9334487c068b6b72699c7")
    uniq_guid = bytes.fromhex("112233445566778899aabbccddeeff01")
    name_utf16 = "FipSOs FAT32 Data".encode("utf-16-le")[:72]
    struct.pack_into(
        "<16s16sQQQ", entries, 0, type_guid, uniq_guid, part_start, part_end, 0
    )
    entries[56 : 56 + len(name_utf16)] = name_utf16
    entries_crc = zlib.crc32(entries) & 0xFFFFFFFF

    # Primary GPT Header (LBA 1) & Backup GPT Header (last LBA)
    def build_gpt_header(my_lba: int, alt_lba: int, entry_lba: int) -> bytearray:
        hdr = bytearray(512)
        disk_guid = bytes.fromhex("99887766554433221100aabbccddeeff")
        struct.pack_into(
            "<8sIIIIQQQQ16sQIII",
            hdr,
            0,
            b"EFI PART",
            0x00010000,
            92,
            0,
            0,
            my_lba,
            alt_lba,
            34,
            total_sectors - 34,
            disk_guid,
            entry_lba,
            128,
            128,
            entries_crc,
        )
        hdr_crc = zlib.crc32(hdr[:92]) & 0xFFFFFFFF
        struct.pack_into("<I", hdr, 16, hdr_crc)
        return hdr

    hdr_primary = build_gpt_header(1, total_sectors - 1, 2)
    hdr_backup = build_gpt_header(total_sectors - 1, 1, total_sectors - 33)

    buf[512:1024] = hdr_primary
    buf[1024 : 1024 + len(entries)] = entries
    buf[(total_sectors - 33) * 512 : (total_sectors - 1) * 512] = entries
    buf[(total_sectors - 1) * 512 : total_sectors * 512] = hdr_backup

    format_fat32_partition(buf, part_start, part_sectors, "desde_host.txt", b"DESDE_~1TXT", host_payload)

    with open(path, "wb") as f:
        f.write(buf)
    return bytes(buf[0:1024]), bytes(entries)


def create_mbr_fat32_disk(path: str, mbr_payload: bytes):
    if os.path.exists(path):
        os.remove(path)
    total_sectors = 131072  # 64 MiB
    buf = bytearray(total_sectors * 512)
    part_start = 2048
    part_sectors = total_sectors - part_start

    # MBR Partition Entry 0: Type 0x0C (FAT32 LBA)
    buf[446:462] = struct.pack(
        "<B3sB3sII", 0x80, b"\x00\x02\x00", 0x0C, b"\xff\xff\xff", part_start, part_sectors
    )
    buf[510:512] = b"\x55\xAA"

    format_fat32_partition(buf, part_start, part_sectors, "desde_mbr.txt", b"DESDE_~1TXT", mbr_payload)
    with open(path, "wb") as f:
        f.write(buf)


def parse_fat32_dir(disk_bytes: bytes, part_off: int, fat1_off: int, data_off: int, start_cluster: int):
    def read_cluster_chain(c: int) -> list[int]:
        chain = []
        seen = set()
        while 2 <= c < 0x0FFFFFF8 and c not in seen:
            seen.add(c)
            chain.append(c)
            entry = struct.unpack_from("<I", disk_bytes, fat1_off + c * 4)[0] & 0x0FFFFFFF
            c = entry
        return chain

    chain = read_cluster_chain(start_cluster)
    raw_dir = bytearray()
    for c in chain:
        coff = data_off + (c - 2) * 512
        raw_dir.extend(disk_bytes[coff : coff + 512])

    entries = {}
    lfn_parts = {}
    for idx in range(0, len(raw_dir), 32):
        rec = raw_dir[idx : idx + 32]
        if rec[0] == 0x00:
            break
        if rec[0] == 0xE5:
            lfn_parts.clear()
            continue
        attr = rec[11]
        if attr == 0x0F:
            ord_val = rec[0] & 0x1F
            csum = rec[13]
            codes = []
            for start, seg in [(1, 5), (14, 6), (28, 2)]:
                for j in range(seg):
                    code = struct.unpack_from("<H", rec, start + j * 2)[0]
                    if code not in (0x0000, 0xFFFF):
                        codes.append(chr(code))
            lfn_parts[ord_val] = (csum, "".join(codes))
            continue

        if attr & 0x08:
            lfn_parts.clear()
            continue

        raw83 = bytes(rec[0:11])
        expected_csum = fat_checksum_83(raw83)
        if lfn_parts and all(v[0] == expected_csum for v in lfn_parts.values()):
            name = "".join(lfn_parts[k][1] for k in sorted(lfn_parts.keys()))
        else:
            base = raw83[0:8].decode("ascii", "replace").rstrip()
            ext = raw83[8:11].decode("ascii", "replace").rstrip()
            name = f"{base}.{ext}".lower() if ext else base.lower()
        lfn_parts.clear()

        hi = struct.unpack_from("<H", rec, 20)[0]
        lo = struct.unpack_from("<H", rec, 26)[0]
        fclus = (hi << 16) | lo
        size = struct.unpack_from("<I", rec, 28)[0]
        entries[name] = {
            "attr": attr,
            "is_dir": bool(attr & 0x10),
            "cluster": fclus,
            "size": size,
            "chain": read_cluster_chain(fclus) if fclus >= 2 else [],
        }
    return entries


def verify_gpt_fat32_image_on_host(
    path: str, orig_lba0_1: bytes, orig_gpt_entries: bytes, expected_host_payload: bytes, expected_fipsos_payload: bytes
):
    with open(path, "rb") as f:
        img = f.read()

    # 1. Verify Protective MBR + GPT header & partition entries are 100% untouched!
    assert img[0:1024] == orig_lba0_1, "CORRUPCION: LBA 0 MBR o LBA 1 GPT Header fueron modificados!"
    assert img[1024 : 1024 + len(orig_gpt_entries)] == orig_gpt_entries, "CORRUPCION: Entradas GPT modificadas!"

    part_off = 2048 * 512
    reserved = struct.unpack_from("<H", img, part_off + 14)[0]
    fat_size = struct.unpack_from("<I", img, part_off + 36)[0]
    root_cluster = struct.unpack_from("<I", img, part_off + 44)[0]

    fat1_off = part_off + reserved * 512
    fat2_off = fat1_off + fat_size * 512
    data_off = fat2_off + fat_size * 512

    # 2. Verify FAT1 == FAT2 bit-for-bit!
    assert (
        img[fat1_off : fat1_off + fat_size * 512] == img[fat2_off : fat2_off + fat_size * 512]
    ), "DESINCRONIZACION FAT1 vs FAT2 detectada tras escrituras!"

    # 3. Parse Root Directory
    root = parse_fat32_dir(img, part_off, fat1_off, data_off, root_cluster)
    assert "desde_host.txt" in root, f"Falta desde_host.txt en raiz: {list(root.keys())}"
    assert "fipsos_out" in root and root["fipsos_out"]["is_dir"], f"Falta directorio fipsos_out: {list(root.keys())}"

    # Verify host file intact
    host_info = root["desde_host.txt"]
    host_bytes = b"".join(img[data_off + (c - 2) * 512 : data_off + (c - 1) * 512] for c in host_info["chain"])[: host_info["size"]]
    assert host_bytes == expected_host_payload, f"Contenido corrupto en desde_host.txt: {host_bytes!r}"

    # 4. Parse fipsos_out directory
    out_dir = parse_fat32_dir(img, part_off, fat1_off, data_off, root["fipsos_out"]["cluster"])
    assert "." in out_dir and out_dir["."]["cluster"] == root["fipsos_out"]["cluster"], "Entrada '.' invalida en fipsos_out"
    assert ".." in out_dir, "Falta entrada '..' en fipsos_out"
    assert "desde_fipsos.txt" in out_dir, f"Falta desde_fipsos.txt en /fipsos_out: {list(out_dir.keys())}"

    fips_info = out_dir["desde_fipsos.txt"]
    assert fips_info["size"] == len(expected_fipsos_payload), (
        f"Tamano incorrecto en desde_fipsos.txt: {fips_info['size']} vs {len(expected_fipsos_payload)}"
    )
    fips_bytes = b"".join(img[data_off + (c - 2) * 512 : data_off + (c - 1) * 512] for c in fips_info["chain"])[: fips_info["size"]]
    assert fips_bytes == expected_fipsos_payload, (
        f"Payload escrito por FipSOs no coincide en host: {fips_bytes!r} vs {expected_fipsos_payload!r}"
    )
    print("[OK] Verificacion estructural FAT32+GPT en Host: GPT intacto, FAT1==FAT2, '.'/'..' OK, LFN OK, bytes 100% integros.")


def test_gpt_fat32_roundtrip_and_ps() -> None:
    print("\n--- [TEST 1] GPT + FAT32 R/W Roundtrip + Escudo Anti-Borrado + ps Limpio + v4.0 Sync ---")
    serial_log = f"{SCRATCH_DIR}/serial_v40_gpt.log"
    qmp_sock = "/tmp/fipsos_v40_gpt_qmp.sock"
    for f in (serial_log, qmp_sock):
        if os.path.exists(f):
            os.remove(f)

    host_payload = b"Mensaje original creado en el Host Windows/Linux GPT+FAT32 (v4.0)!\n"
    fipsos_payload = b"Hola_desde_FipSOs_v40_FAT32\n"
    orig_lba0_1, orig_entries = create_gpt_fat32_disk(GPT_FAT32_IMG, host_payload)

    qemu_cmd = [
        "qemu-system-x86_64",
        "-M", "q35",
        "-cpu", "max",
        "-smp", "2",
        "-m", "512M",
        "-no-reboot", "-no-shutdown",
        "-serial", f"file:{serial_log}",
        "-qmp", f"unix:{qmp_sock},server,nowait",
        "-drive", f"id=satadisk,file={GPT_FAT32_IMG},format=raw,if=none",
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

        assert "[FipSOs] Framekernel FipSOs v4.0 activo" in serial, "Falta banner kernel FipSOs v4.0"
        assert "Tabla GPT/MBR externa detectada - Modo Protegido (Sin formateo)" in serial, (
            "Falta Escudo Anti-Borrado GPT/MBR Modo Protegido"
        )
        assert "[FAT32] Particion GPT" in serial and "montada en /mnt/fat0" in serial, (
            "Falta montaje automatico FAT32 GPT en /mnt/fat0"
        )
        assert "FIPSOS_DISKFS_V2" not in serial and "Formateando disco" not in serial, (
            "Se intento formatear disco GPT FAT32!"
        )
        print("[OK] Kernel v4.0 + Escudo GPT + Auto-montaje /mnt/fat0 verificados en serial.")

        qmp = QmpClient(qmp_sock)
        time.sleep(0.4)
        ppm1 = f"{SCRATCH_DIR}/shot1_v40_boot.ppm"
        qmp.screendump(ppm1)
        ppm_to_png(ppm1, f"{ARTIFACT_DIR}/shot1_v40_boot_mount.png")

        # 1. Read host file from FAT32
        qmp.type_string("ls /mnt/fat0\n", delay=0.03)
        serial = wait_for_serial(serial_log, "desde_host.txt", 8.0)

        qmp.type_string("cat /mnt/fat0/desde_host.txt\n", delay=0.03)
        serial = wait_for_serial(serial_log, "Mensaje original creado en el Host Windows/Linux GPT+FAT32 (v4.0)!", 8.0)

        # 2. Create directory & write file from FipSOs to FAT32
        qmp.type_string("mkdir /mnt/fat0/fipsos_out\n", delay=0.05)
        wait_for_serial(serial_log, "creado", 8.0)
        qmp.type_string("echo Hola_desde_FipSOs_v40_FAT32 > /mnt/fat0/fipsos_out/desde_fipsos.txt\n", delay=0.05)
        wait_for_serial(serial_log, "escrito en '/mnt/fat0/fipsos_out/desde_fipsos.txt'", 8.0)
        qmp.type_string("ls /mnt/fat0/fipsos_out\n", delay=0.05)
        wait_for_serial(serial_log, "28 B", 8.0)
        qmp.type_string("cat /mnt/fat0/fipsos_out/desde_fipsos.txt\n", delay=0.05)
        serial = wait_for_serial(serial_log, "cat /mnt/fat0/fipsos_out/desde_fipsos.txt", 8.0)
        after_cat = serial[serial.rfind("cat /mnt/fat0/fipsos_out/desde_fipsos.txt") :]
        deadline = time.time() + 6.0
        while "Hola_desde_FipSOs_v40_FAT32" not in after_cat.splitlines()[1:]:
            if time.time() > deadline:
                raise AssertionError(f"cat no mostro contenido desde FAT32:\n{after_cat}")
            time.sleep(0.2)
            serial = strip_ansi(read_serial(serial_log))
            after_cat = serial[serial.rfind("cat /mnt/fat0/fipsos_out/desde_fipsos.txt") :]

        time.sleep(0.4)
        ppm2 = f"{SCRATCH_DIR}/shot2_v40_fat32.ppm"
        qmp.screendump(ppm2)
        ppm_to_png(ppm2, f"{ARTIFACT_DIR}/shot2_v40_fat32_ops.png")

        # 3. Verify /proc/mounts, child execution roundtrip (goose_mon), and clean `ps`
        qmp.type_string("cat /proc/mounts\n", delay=0.05)
        serial = wait_for_serial(serial_log, "/dev/block0 /mnt/fat0 vfat rw 0 0", 8.0)

        qmp.type_string("goose_mon\n", delay=0.05)
        wait_for_serial(serial_log, "GOOSE CAPA 3 MONITOR (FipSOs v4.0", 8.0)

        qmp.type_string("ps\n", delay=0.05)
        serial = wait_for_serial(serial_log, "npu-telemetryd", 8.0)
        ps_idx = serial.rfind("[Scheduler::ps]")
        ps_block = serial[ps_idx:] if ps_idx != -1 else serial[-500:]
        assert "fipsos_sh" in ps_block and "Running" in ps_block, f"fipsos_sh debe seguir Running:\n{ps_block}"
        assert "npu-telemetryd" in ps_block, f"Falta npu-telemetryd en salida ps:\n{ps_block}"
        assert "goose_mon" not in ps_block, f"Residuo de proceso hijo terminado en ps:\n{ps_block}"
        assert "PID=2000" not in ps_block and "SLOT=6" not in ps_block, (
            f"Slot hijo forkeado huerfano en ps:\n{ps_block}"
        )
        assert "fipsos_sh.elf" not in ps_block and "npu-telemetryd.sh" not in ps_block, (
            f"Residuo de nombre antiguo en ps:\n{ps_block}"
        )

        # Verify TCP :2222 v4.0 banner
        sock = socket.create_connection(("127.0.0.1", 2222), timeout=5.0)
        banner = sock.recv(512).decode("utf-8", errors="ignore")
        sock.close()
        assert "FipSOs v4.0 Remote Shell (TCP :2222)" in banner, f"Banner remoto incorrecto: {banner!r}"

        time.sleep(0.4)
        ppm3 = f"{SCRATCH_DIR}/shot3_v40_ps.ppm"
        qmp.screendump(ppm3)
        ppm_to_png(ppm3, f"{ARTIFACT_DIR}/shot3_v40_ps_clean.png")
        print("[OK] R/W FAT32 en /mnt/fat0, /proc/mounts, ps limpio y TCP :2222 v4.0 verificados.")

    finally:
        if qmp is not None:
            qmp.quit()
        proc.terminate()
        try:
            proc.wait(timeout=5)
        except Exception:
            proc.kill()

    verify_gpt_fat32_image_on_host(GPT_FAT32_IMG, orig_lba0_1, orig_entries, host_payload, fipsos_payload)


def test_mbr_fat32_mount_read() -> None:
    print("\n--- [TEST 2] Parser de Particion MBR (Tipo 0x0C FAT32 LBA) en /mnt/fat0 ---")
    serial_log = f"{SCRATCH_DIR}/serial_v40_mbr.log"
    qmp_sock = "/tmp/fipsos_v40_mbr_qmp.sock"
    for f in (serial_log, qmp_sock):
        if os.path.exists(f):
            os.remove(f)

    mbr_payload = b"Leido desde particion MBR Tipo 0x0C en FipSOs v4.0!\n"
    create_mbr_fat32_disk(MBR_FAT32_IMG, mbr_payload)

    qemu_cmd = [
        "qemu-system-x86_64",
        "-M", "q35",
        "-cpu", "max",
        "-smp", "2",
        "-m", "512M",
        "-no-reboot", "-no-shutdown",
        "-serial", f"file:{serial_log}",
        "-qmp", f"unix:{qmp_sock},server,nowait",
        "-drive", f"id=satadisk,file={MBR_FAT32_IMG},format=raw,if=none",
        "-device", "ide-hd,drive=satadisk,bus=ide.0",
        "-boot", "d",
        "-cdrom", ISO_IMG,
    ]
    proc = subprocess.Popen(qemu_cmd)
    qmp = None
    try:
        serial = wait_for_serial(serial_log, "for available commands", 20.0)
        assert "[FAT32] Particion MBR" in serial and "montada en /mnt/fat0" in serial, (
            "Falta deteccion/montaje MBR 0x0C en /mnt/fat0"
        )
        assert "Tabla GPT/MBR externa detectada - Modo Protegido (Sin formateo)" in serial, (
            "Falta Modo Protegido ante tabla MBR!"
        )

        qmp = QmpClient(qmp_sock)
        qmp.type_string("cat /mnt/fat0/desde_mbr.txt\n", delay=0.03)
        wait_for_serial(serial_log, "Leido desde particion MBR Tipo 0x0C en FipSOs v4.0!", 8.0)
        print("[OK] Deteccion MBR 0x0C y lectura /mnt/fat0/desde_mbr.txt verificadas.")
    finally:
        if qmp is not None:
            qmp.quit()
        proc.terminate()
        try:
            proc.wait(timeout=5)
        except Exception:
            proc.kill()


if __name__ == "__main__":
    test_gpt_fat32_roundtrip_and_ps()
    test_mbr_fat32_mount_read()
    print("\n=== TODAS LAS PRUEBAS FASE v4.0 (GPT/MBR + FAT32 R/W + ps + Sync) SUPERADAS CON EXITO ===")
