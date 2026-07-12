use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::net::TcpListener;
use tracing::info;

/// Shared readiness flags for agent /health and /ready (SPEC §19).
#[derive(Clone, Default)]
pub struct AgentReady {
    pub tailscale_ok: Arc<AtomicBool>,
    pub docker_ok: Arc<AtomicBool>,
    pub proxy_ok: Arc<AtomicBool>,
    pub enrolled: Arc<AtomicBool>,
    pub initial_reconcile: Arc<AtomicBool>,
}

impl AgentReady {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_ready(&self) -> bool {
        self.tailscale_ok.load(Ordering::Relaxed)
            && self.docker_ok.load(Ordering::Relaxed)
            && self.proxy_ok.load(Ordering::Relaxed)
            && self.enrolled.load(Ordering::Relaxed)
            && self.initial_reconcile.load(Ordering::Relaxed)
    }

    pub fn set(&self, flag: &Arc<AtomicBool>, v: bool) {
        flag.store(v, Ordering::Relaxed);
    }
}

/// Minimal status server (not on the Tailscale app proxy path).
pub async fn run_status_server(
    listen: std::net::SocketAddr,
    ready: AgentReady,
) -> std::io::Result<()> {
    let listener = TcpListener::bind(listen).await?;
    info!(addr = %listen, "agent status listening");
    loop {
        let (mut stream, _) = listener.accept().await?;
        let ready = ready.clone();
        tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let mut buf = [0u8; 1024];
            let _ = stream.read(&mut buf).await;
            let req = String::from_utf8_lossy(&buf);
            let path = req.lines().next().unwrap_or("");
            let (code, body) = if path.contains(" /ready") {
                if ready.is_ready() {
                    ("200 OK", "ready")
                } else {
                    ("503 Service Unavailable", "not ready")
                }
            } else {
                // /health and anything else
                ("200 OK", "ok")
            };
            let resp = format!(
                "HTTP/1.1 {code}\r\ncontent-type: text/plain\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(resp.as_bytes()).await;
        });
    }
}
