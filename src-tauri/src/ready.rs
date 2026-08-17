use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

const EXCHANGE: Duration = Duration::from_millis(400);

/// True when the loopback server is the dsh Web UI, not just any HTTP listener:
/// the index carries the boot graph, and `host.describe` answers ok.
pub fn dsh_serving(host: &str, port: u16) -> bool {
    index_has_boot(host, port) && host_describes(host, port)
}

fn index_has_boot(host: &str, port: u16) -> bool {
    let request = format!("GET / HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
    http_body(host, port, &request).is_some_and(|body| index_ready(&body))
}

fn host_describes(host: &str, port: u16) -> bool {
    let payload =
        r#"{"type":"client-request","rpcId":"oardsh-ready","method":"host.describe","payload":{}}"#;
    let request = format!(
        "POST /api/host.describe HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
        payload.len()
    );
    http_body(host, port, &request).is_some_and(|body| describe_ready(&body))
}

fn http_body(host: &str, port: u16, request: &str) -> Option<String> {
    let address = format!("{host}:{port}");
    let addr = address.to_socket_addrs().ok()?.next()?;
    let mut stream = TcpStream::connect_timeout(&addr, EXCHANGE).ok()?;
    let _ = stream.set_read_timeout(Some(EXCHANGE));
    let _ = stream.set_write_timeout(Some(EXCHANGE));
    stream.write_all(request.as_bytes()).ok()?;
    let mut bytes = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => bytes.extend_from_slice(&buf[..n]),
            Err(_) => break,
        }
        if bytes.len() > 512 * 1024 {
            break;
        }
    }
    let text = String::from_utf8_lossy(&bytes);
    let (headers, rest) = text.split_once("\r\n\r\n")?;
    if !headers.starts_with("HTTP/") {
        return None;
    }
    let status = headers
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse::<u16>().ok())?;
    if !(200..400).contains(&status) {
        return None;
    }
    let chunked = headers.lines().any(|line| {
        line.to_ascii_lowercase().starts_with("transfer-encoding:")
            && line.to_ascii_lowercase().contains("chunked")
    });
    if chunked {
        decode_chunked(rest.as_bytes())
            .map(|decoded| String::from_utf8_lossy(&decoded).into_owned())
    } else {
        Some(rest.to_string())
    }
}

pub(crate) fn index_ready(body: &str) -> bool {
    body.contains("__DSH_BOOT__")
}

pub(crate) fn describe_ready(body: &str) -> bool {
    body.contains("\"ok\":true") || body.contains("\"ok\": true")
}

pub(crate) fn decode_chunked(bytes: &[u8]) -> Option<Vec<u8>> {
    let mut i = 0;
    let mut out = Vec::new();
    while i < bytes.len() {
        let rel = bytes[i..].iter().position(|&b| b == b'\n')?;
        let line = std::str::from_utf8(&bytes[i..i + rel]).ok()?.trim();
        let size = usize::from_str_radix(line.split(';').next()?, 16).ok()?;
        i += rel + 1;
        if size == 0 {
            return Some(out);
        }
        if i + size > bytes.len() {
            return None;
        }
        out.extend_from_slice(&bytes[i..i + size]);
        i += size;
        if bytes.get(i) == Some(&b'\r') {
            i += 1;
        }
        if bytes.get(i) == Some(&b'\n') {
            i += 1;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{decode_chunked, describe_ready, index_ready};

    #[test]
    fn index_requires_the_boot_graph() {
        assert!(index_ready("<script>window.__DSH_BOOT__ = {}</script>"));
        assert!(!index_ready("<html>ok</html>"));
    }

    #[test]
    fn describe_requires_ok_true() {
        assert!(describe_ready(
            r#"{"type":"server-response","result":{"ok":true,"value":{}}}"#
        ));
        assert!(!describe_ready(
            r#"{"type":"server-response","result":{"ok":false}}"#
        ));
    }

    #[test]
    fn decodes_chunked_bodies() {
        let raw = b"b\r\n{\"ok\":true}\r\n0\r\n\r\n";
        let decoded = decode_chunked(raw).unwrap();
        assert_eq!(decoded, br#"{"ok":true}"#);
        assert!(describe_ready(&String::from_utf8(decoded).unwrap()));
    }
}
