//! Controller process entry.
//!
//! Design goals:
//! - Never panic-abort the process from recoverable faults
//! - DNS / cleanup / backup tasks self-restart
//! - API panics are caught per-request (CatchPanicLayer)

use anyhow::Context;
use chrono::Duration as ChronoDuration;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tailsvc_controller::config::ControllerConfig;
use tailsvc_controller::refresh_dns_registry;
use tailsvc_controller::state::AppState;
use tailsvc_dns::{DnsRegistry, DnsServer, DnsServerConfig};
use tokio::net::TcpListener;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

fn install_panic_hook() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // Log and continue — do not abort. Combined with CatchPanicLayer for HTTP.
        error!(panic = %info, "controller panic caught (process continues)");
        prev(info);
    }));
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    install_panic_hook();

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let cfg = ControllerConfig::load().context("load config")?;

    // Ensure parent dir for sqlite exists
    if let Some(parent) = std::path::Path::new(&cfg.storage.sqlite_path).parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create storage dir {}", parent.display()))?;
    }

    let storage =
        tailsvc_storage::Storage::connect(&cfg.storage.sqlite_path, cfg.leases.agent_ttl_seconds)
            .await
            .context("storage")?;

    let registry = DnsRegistry::new();
    let state = Arc::new(
        AppState::new(storage.clone(), registry.clone(), cfg.clone()).context("app state")?,
    );

    if let Err(e) = refresh_dns_registry(&state).await {
        // Non-fatal at boot: empty registry until agents heartbeat
        warn!(error = %e, "initial dns registry refresh failed");
    }

    // --- lease cleanup (errors logged; loop never panics out) ---
    {
        let ttl = ChronoDuration::seconds(cfg.leases.agent_ttl_seconds as i64);
        let st = state.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(10));
            loop {
                interval.tick().await;
                if let Err(e) = st.storage.purge_expired_agents(ttl).await {
                    warn!(error = %e, "purge_expired_agents");
                }
                if let Err(e) = refresh_dns_registry(&st).await {
                    warn!(error = %e, "refresh_dns_registry");
                }
            }
        });
    }

    // --- DNS server with auto-restart ---
    {
        let st = state.clone();
        let dns_listen = cfg.dns.listen;
        let upstreams = cfg.dns.upstreams.clone();
        let service_ttl = cfg.dns.service_ttl_seconds;
        let upstream_timeout = std::time::Duration::from_millis(cfg.dns.upstream_timeout_ms);
        let positive = std::time::Duration::from_secs(cfg.dns.positive_cache_max_seconds);
        let negative = std::time::Duration::from_secs(cfg.dns.negative_cache_max_seconds);
        let query_timeout = std::time::Duration::from_millis(cfg.dns.query_timeout_ms);
        let registry = registry.clone();
        tokio::spawn(async move {
            let mut backoff = std::time::Duration::from_secs(1);
            loop {
                let mut dns_cfg = DnsServerConfig::with_defaults(
                    dns_listen,
                    upstreams.clone(),
                    service_ttl,
                    upstream_timeout,
                    registry.clone(),
                );
                dns_cfg.positive_cache_max = positive;
                dns_cfg.negative_cache_max = negative;
                dns_cfg.query_timeout = query_timeout;

                match DnsServer::new(dns_cfg).await {
                    Ok(dns) => {
                        st.dns_up.store(true, Ordering::SeqCst);
                        info!(addr = %dns_listen, "dns listening");
                        backoff = std::time::Duration::from_secs(1);
                        if let Err(e) = dns.run().await {
                            error!(error = %e, "dns server exited");
                        }
                        st.dns_up.store(false, Ordering::SeqCst);
                    }
                    Err(e) => {
                        st.dns_up.store(false, Ordering::SeqCst);
                        error!(error = %e, "dns server failed to start");
                    }
                }
                warn!(?backoff, "restarting dns server");
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(std::time::Duration::from_secs(60));
            }
        });
    }

    // --- backup loop (feature-gated) ---
    #[cfg(feature = "backup")]
    if cfg.backup.enabled {
        use tailsvc_controller::backup::{run_backup_loop, BackupConfig};
        let bcfg = BackupConfig {
            dir: cfg.backup.dir.clone(),
            interval: std::time::Duration::from_secs(cfg.backup.interval_seconds.max(60)),
            keep: cfg.backup.keep,
        };
        let storage = storage.clone();
        tokio::spawn(async move {
            run_backup_loop(storage, bcfg).await;
        });
        info!(
            dir = %cfg.backup.dir.display(),
            interval_s = cfg.backup.interval_seconds,
            keep = cfg.backup.keep,
            "sqlite backup enabled"
        );
    }

    let app = tailsvc_controller::api::router(state.clone());
    let listener = TcpListener::bind(cfg.api.listen)
        .await
        .with_context(|| format!("bind api {}", cfg.api.listen))?;
    info!(addr = %cfg.api.listen, "api listening");

    // Serve until fatal accept error; then return Err so systemd/docker restarts us.
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("api serve")?;
    info!("controller shutdown complete");
    Ok(())
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut sigterm = match signal(SignalKind::terminate()) {
            Ok(s) => s,
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
                return;
            }
        };
        let mut sigint = match signal(SignalKind::interrupt()) {
            Ok(s) => s,
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
                return;
            }
        };
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
