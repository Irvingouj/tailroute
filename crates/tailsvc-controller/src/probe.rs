//! Lightweight HTTP probe of a registered route via the Agent data path.
//!
//! Connects to `http://{agent_tailscale_ip}:80/` with `Host: {hostname}`.
//! This is a *semi* health check: registration lease is authoritative for DNS;
//! this only tells whether something answers on the proxy path.

use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

#[derive(Debug, Clone)]
pub struct ProbeResult {
    pub status: String,
    pub ms: Option<u64>,
    pub detail: Option<String>,
}

pub async fn probe_via_agent(agent_ip: &str, hostname: &str, timeout: Duration) -> ProbeResult {
    let addr = format!("{agent_ip}:80");
    let start = Instant::now();
    let connect = tokio::time::timeout(timeout, TcpStream::connect(&addr)).await;
    let mut stream = match connect {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => {
            return ProbeResult {
                status: "fail".into(),
                ms: Some(start.elapsed().as_millis() as u64),
                detail: Some(format!("connect: {e}")),
            };
        }
        Err(_) => {
            return ProbeResult {
                status: "timeout".into(),
                ms: Some(timeout.as_millis() as u64),
                detail: Some("connect timeout".into()),
            };
        }
    };

    // Prefer HEAD; some apps only implement GET — either is fine for "semi" health.
    let req = format!(
        "GET / HTTP/1.1\r\nHost: {hostname}\r\nConnection: close\r\nUser-Agent: tailsvc-admin-probe\r\n\r\n"
    );
    if let Err(e) = stream.write_all(req.as_bytes()).await {
        return ProbeResult {
            status: "fail".into(),
            ms: Some(start.elapsed().as_millis() as u64),
            detail: Some(format!("write: {e}")),
        };
    }

    let mut buf = [0u8; 256];
    let read = tokio::time::timeout(timeout, stream.read(&mut buf)).await;
    let n = match read {
        Ok(Ok(n)) => n,
        Ok(Err(e)) => {
            return ProbeResult {
                status: "fail".into(),
                ms: Some(start.elapsed().as_millis() as u64),
                detail: Some(format!("read: {e}")),
            };
        }
        Err(_) => {
            return ProbeResult {
                status: "timeout".into(),
                ms: Some(timeout.as_millis() as u64),
                detail: Some("read timeout".into()),
            };
        }
    };
    let ms = start.elapsed().as_millis() as u64;
    if n == 0 {
        return ProbeResult {
            status: "fail".into(),
            ms: Some(ms),
            detail: Some("empty response".into()),
        };
    }
    let head = String::from_utf8_lossy(&buf[..n]);
    let line = head.lines().next().unwrap_or("").trim();
    // HTTP/1.x 2xx/3xx => ok; 4xx/5xx still means proxy/path is alive.
    if line.starts_with("HTTP/1.") {
        let code = line.split_whitespace().nth(1).unwrap_or("");
        let ok = code.starts_with('2') || code.starts_with('3') || code.starts_with('4');
        // 5xx from agent (502/504) => backend issue but path works — mark warn-ish as fail detail
        if code.starts_with('5') {
            return ProbeResult {
                status: "fail".into(),
                ms: Some(ms),
                detail: Some(line.to_string()),
            };
        }
        if ok || !code.is_empty() {
            return ProbeResult {
                status: "ok".into(),
                ms: Some(ms),
                detail: Some(line.to_string()),
            };
        }
    }
    ProbeResult {
        status: "fail".into(),
        ms: Some(ms),
        detail: Some(format!("bad status line: {line}")),
    }
}
