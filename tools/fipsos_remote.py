#!/usr/bin/env python3
"""
FipSOs - Dedicated Remote Console Client (tools/fipsos_remote.py)
Interactive TCP client connecting to FipSOs Remote Shell (:2222).
Auto-detects environment (Native Linux, WSL2, Windows), supports right-click paste,
and restores terminal attributes cleanly upon exit.
"""

import os
import sys
import socket
import threading
import time

def find_wsl_host_ip():
    """Detects Windows host IP from /etc/resolv.conf in WSL2."""
    if os.path.exists("/etc/resolv.conf"):
        try:
            with open("/etc/resolv.conf", "r") as f:
                for line in f:
                    line = line.strip()
                    if line.startswith("nameserver"):
                        parts = line.split()
                        if len(parts) >= 2:
                            return parts[1]
        except Exception:
            pass
    return None

def connect_fipsos(target_host=None, target_port=2222):
    """Attempts connection to FipSOs TCP remote shell with automatic fallback."""
    candidates = []
    if target_host:
        candidates.append(target_host)
    else:
        candidates.append("127.0.0.1")
        wsl_ip = find_wsl_host_ip()
        if wsl_ip and wsl_ip != "127.0.0.1":
            candidates.append(wsl_ip)

    for host in candidates:
        print(f"[fipsos_remote] Probando conexion con {host}:{target_port}...")
        try:
            s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            s.settimeout(2.0)
            s.connect((host, target_port))
            s.settimeout(None)
            print(f"\033[1;32m[fipsos_remote] Conectado exitosamente a FipSOs en {host}:{target_port}\033[0m")
            return s
        except Exception as e:
            print(f"[fipsos_remote] No se pudo conectar a {host}:{target_port}: {e}")

    return None

def run_interactive(sock):
    """Runs a transparent bidirectional interactive terminal loop."""
    stop_event = threading.Event()

    def recv_thread_func():
        while not stop_event.is_set():
            try:
                data = sock.recv(4096)
                if not data:
                    print("\n[fipsos_remote] El servidor cerro la conexion.")
                    stop_event.set()
                    break
                sys.stdout.buffer.write(data)
                sys.stdout.buffer.flush()
            except Exception:
                stop_event.set()
                break

    recv_thread = threading.Thread(target=recv_thread_func, daemon=True)
    recv_thread.start()

    # POSIX terminal setup (Linux / WSL2 / macOS)
    old_tty_settings = None
    if sys.platform != "win32" and sys.stdin.isatty():
        try:
            import termios
            import tty
            old_tty_settings = termios.tcgetattr(sys.stdin.fileno())
            tty.setraw(sys.stdin.fileno())
        except Exception:
            pass

    try:
        if sys.platform == "win32":
            import msvcrt
            while not stop_event.is_set():
                if msvcrt.kbhit():
                    ch = msvcrt.getch()
                    if ch == b"\x03":  # Ctrl+C
                        break
                    sock.sendall(ch)
                else:
                    time.sleep(0.01)
        else:
            fd = sys.stdin.fileno()
            while not stop_event.is_set():
                try:
                    chunk = os.read(fd, 1024)
                    if not chunk:
                        break
                    if b"\x03" in chunk:  # Ctrl+C
                        sock.sendall(b"\x03")
                        break
                    sock.sendall(chunk)
                except (IOError, OSError):
                    break
    except KeyboardInterrupt:
        pass
    finally:
        stop_event.set()
        if old_tty_settings:
            try:
                import termios
                termios.tcsetattr(sys.stdin.fileno(), termios.TCSADRAIN, old_tty_settings)
            except Exception:
                pass
        try:
            sock.close()
        except Exception:
            pass
        print("\r\n\033[1;33m[fipsos_remote] Desconectado de FipSOs Remote Console. Sesion finalizada.\033[0m\r\n")

def main():
    host = sys.argv[1] if len(sys.argv) > 1 else None
    port = int(sys.argv[2]) if len(sys.argv) > 2 else 2222

    print("\033[1;36m======================================================================\033[0m")
    print("\033[1;32m  FipSOs Remote Terminal Client (TCP :2222)\033[0m")
    print("  Use Ctrl+C para salir limpiamente. Soporta pegado con clic derecho.")
    print("\033[1;36m======================================================================\033[0m")

    sock = connect_fipsos(host, port)
    if not sock:
        print("\033[1;31m[fipsos_remote] Error: No se pudo conectar a ninguna direccion de FipSOs.\033[0m")
        print("Asegurese de que la maquina virtual QEMU o VirtualBox este en ejecucion con port-forwarding TCP 2222 -> 2222.")
        sys.exit(1)

    run_interactive(sock)

if __name__ == "__main__":
    main()
