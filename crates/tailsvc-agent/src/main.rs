mod agent_state;
mod config;
mod controller_client;
mod ready;
mod reconcile;
mod static_routes;
mod tailscale;

use anyhow::Context;
use config::AgentConfig;
use ready::AgentReady;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tailsvc_proxy::{ProxyConfig, ProxyServer, SharedRouteStore};
use tracing::info;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let cfg = AgentConfig::load()?;
    let ready = AgentReady::new();

    let tailscale_ip = tailscale::resolve_tailscale_ipv4(&cfg).context("tailscale ip")?;
    ready.set(&ready.tailscale_ok, true);

    let listen: SocketAddr = match tailscale::proxy_listen_addr(&cfg, tailscale_ip) {
        Ok(a) => a,
        Err(e) => {
            anyhow::bail!(
                "failed to resolve proxy listen address for {tailscale_ip}: {e} \
                 (port {} unavailable? no automatic fallback)",
                cfg.proxy_port
            );
        }
    };

    if let Some(ref status) = cfg.status_listen {
        let addr: SocketAddr = status
            .parse()
            .with_context(|| format!("invalid status_listen: {status}"))?;
        let r = ready.clone();
        tokio::spawn(async move {
            if let Err(e) = ready::run_status_server(addr, r).await {
                tracing::error!(error = %e, "status server exited");
            }
        });
    }

    let routes = Arc::new(SharedRouteStore::new());
    let shutting_down = Arc::new(AtomicBool::new(false));
    let proxy = ProxyServer::new(ProxyConfig {
        listen,
        connect_timeout: Duration::from_millis(cfg.connect_timeout_ms),
        response_timeout: Duration::from_secs(cfg.response_timeout_seconds),
        routes: routes.clone(),
        shutting_down: shutting_down.clone(),
    });

    let proxy_task = tokio::spawn(async move {
        if let Err(e) = proxy.run().await {
            tracing::error!(error = %e, "proxy exited (bind or runtime failure)");
        }
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    if proxy_task.is_finished() {
        anyhow::bail!(
            "proxy failed to bind to {listen} — port unavailable; no automatic fallback (SPEC §5.1)"
        );
    }
    ready.set(&ready.proxy_ok, true);
    info!(addr = %listen, "proxy bound");

    let shutting_down_signal = shutting_down.clone();
    tokio::spawn(async move {
        shutdown_signal().await;
        info!("shutdown signal received; draining proxy");
        shutting_down_signal.store(true, Ordering::SeqCst);
    });

    let mut run = reconcile::AgentRun::new(cfg, routes, tailscale_ip, ready).await?;
    run.loop_forever().await
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut sigterm = signal(SignalKind::terminate()).expect("sigterm");
        let mut sigint = signal(SignalKind::interrupt()).expect("sigint");
        tokio::select! {
            _ = sigterm.recv() => {}
            _ = sigint.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
