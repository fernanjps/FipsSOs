#![forbid(unsafe_code)]

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use ostd::sync::SpinLock;

static BG_HTTPD_STATE: SpinLock<Option<(u64, u16)>> = SpinLock::new(None);

/// Registers a listening TCP socket as the non-blocking background HTTP daemon (`/mnt/fat0`).
pub fn start_bg_httpd(sock_id: u64, port: u16) -> i64 {
    let pid = ostd::scheduler::active_pid();
    *BG_HTTPD_STATE.lock() = Some((sock_id, port));
    ostd::scheduler::register_bg_httpd(pid, port);
    ostd::net::register_bg_net_poll_hook(poll_bg_httpd);
    ostd::early_println!(
        "[linux_abi::httpd_bg] Demonio HTTP/1.0 activo en segundo plano (PID={}, port={})",
        pid,
        port
    );
    0
}

fn send_http_resp(cli: u64, status: &str, ctype: &str, body: &[u8]) {
    let hdr = format!(
        "HTTP/1.0 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        status,
        ctype,
        body.len()
    );
    let _ = ostd::net::tcp::send(cli, hdr.as_bytes());
    if !body.is_empty() {
        let _ = ostd::net::tcp::send(cli, body);
    }
}

fn parse_content_length(hdr_str: &str) -> usize {
    for line in hdr_str.split("\r\n") {
        let lower = line.to_ascii_lowercase();
        if let Some(rest) = lower.strip_prefix("content-length:") {
            if let Ok(v) = rest.trim().parse::<usize>() {
                return v.min(65536);
            }
        }
    }
    0
}

fn handle_bg_http_client(cli: u64) {
    let mut raw = Vec::<u8>::new();
    let mut chunk = [0u8; 2048];
    let mut hdr_end: Option<usize> = None;

    for _ in 0..8 {
        match ostd::net::tcp::recv(cli, &mut chunk) {
            Ok(n) if n > 0 => {
                raw.extend_from_slice(&chunk[..n]);
                if hdr_end.is_none() {
                    if let Some(pos) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                        hdr_end = Some(pos + 4);
                    }
                }
                if let Some(he) = hdr_end {
                    let hdr_str = core::str::from_utf8(&raw[..he]).unwrap_or("");
                    let clen = parse_content_length(hdr_str);
                    if raw.len() >= he + clen {
                        break;
                    }
                }
            }
            _ => break,
        }
    }

    let Some(he) = hdr_end else {
        let _ = ostd::net::tcp::close(cli);
        return;
    };
    let hdr_str = core::str::from_utf8(&raw[..he]).unwrap_or("");
    let first_line = hdr_str.lines().next().unwrap_or("");
    let mut parts = first_line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let uri = parts.next().unwrap_or("/");

    if uri.contains("..") {
        send_http_resp(cli, "403 Forbidden", "text/plain", b"Path traversal rejected\n");
        let _ = ostd::net::tcp::close(cli);
        return;
    }

    if method == "GET" {
        let clean_uri = uri.split('?').next().unwrap_or(uri);
        if clean_uri == "/" || clean_uri == "/index.html" {
            let html = format!(
                "<html><body><h1>FipSOs v6.3 HTTP File Server</h1>\
                 <p>Daemon activo en segundo plano sobre <code>/mnt/fat0/</code>.</p>\
                 <p>GET /&lt;archivo&gt; | POST /upload?file=&lt;nombre&gt;</p></body></html>\n"
            );
            send_http_resp(cli, "200 OK", "text/html; charset=utf-8", html.as_bytes());
        } else {
            let rel = clean_uri.trim_start_matches('/');
            let path = format!("/mnt/fat0/{}", rel);
            match crate::fat32::try_read_file(&path) {
                Ok(data) => send_http_resp(cli, "200 OK", "text/plain; charset=utf-8", &data),
                Err("Volumen FAT32 ocupado") => send_http_resp(cli, "503 Service Unavailable", "text/plain", b"FAT32 busy\n"),
                Err(_) => send_http_resp(cli, "404 Not Found", "text/plain", b"Not Found\n"),
            }
        }
    } else if method == "POST" {
        let mut target_name = String::new();
        if let Some((_, query)) = uri.split_once('?') {
            for kv in query.split('&') {
                if let Some(val) = kv.strip_prefix("file=") {
                    target_name = String::from(val);
                    break;
                }
            }
        }
        if target_name.is_empty() || target_name.contains("..") || target_name.contains('/') {
            send_http_resp(cli, "400 Bad Request", "text/plain", b"Invalid upload filename\n");
            let _ = ostd::net::tcp::close(cli);
            return;
        }
        let clen = parse_content_length(hdr_str);
        let body_avail = raw.len().saturating_sub(he);
        let write_len = if clen > 0 { clen.min(body_avail) } else { body_avail };
        let path = format!("/mnt/fat0/{}", target_name);
        match crate::fat32::try_write_file(&path, &raw[he..he + write_len]) {
            Ok(()) => {
                ostd::early_println!(
                    "[linux_abi::httpd_bg] POST /upload -> {} ({} B)",
                    path,
                    write_len
                );
                let msg = format!("{{\"ok\":true,\"file\":\"{}\",\"bytes\":{}}}\n", target_name, write_len);
                send_http_resp(cli, "200 OK", "application/json", msg.as_bytes());
            }
            Err(_) => {
                send_http_resp(cli, "500 Internal Server Error", "text/plain", b"FAT32 write failed\n");
            }
        }
    } else {
        send_http_resp(cli, "405 Method Not Allowed", "text/plain", b"Method Not Allowed\n");
    }

    let _ = ostd::net::tcp::close(cli);
}

/// Non-blocking AP worker poll entrypoint (outside IRQ) for the background HTTP daemon.
pub fn poll_bg_httpd() {
    let Some(guard) = BG_HTTPD_STATE.try_lock() else {
        return;
    };
    let Some((srv_sock, _port)) = *guard else {
        return;
    };
    drop(guard);

    let (active, _, _) = ostd::scheduler::bg_httpd_status();
    if !active {
        if let Some(mut guard) = BG_HTTPD_STATE.try_lock() {
            if let Some((sock, _)) = guard.take() {
                ostd::net::tcp::unbind(sock);
            }
        }
        return;
    }

    if let Some(cli) = ostd::net::tcp::try_accept_nonblocking(srv_sock) {
        handle_bg_http_client(cli);
    }
}
