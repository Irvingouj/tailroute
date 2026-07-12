pub mod api;
#[cfg(feature = "backup")]
pub mod backup;
pub mod config;
pub mod probe;
pub mod state;

use anyhow::Context;
use chrono::Duration as ChronoDuration;
use config::ControllerConfig;
use state::{AppState, SharedState};
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use tailsvc_dns::{DnsRegistry, DnsServer, DnsServerConfig};
use tailsvc_storage::Storage;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

/// In-memory controller stack for tests and embedding.
pub struct ControllerBootstrap {
    pub state: SharedState,
    pub api_listen: SocketAddr,
    pub dns_listen: SocketAddr,
    _api_task: JoinHandle<()>,
    _dns_task: JoinHandle<()>,
    _cleanup_task: JoinHandle<()>,
}

impl ControllerBootstrap {
    pub async fn start_test(sqlite_path: &str, admin_token: &str) -> anyhow::Result<Self> {
        let api_listen = free_port()?;
        let dns_listen = free_port()?;

        let cfg = ControllerConfig {
            dns: config::DnsConfig {
                listen: dns_listen,
                upstreams: vec!["1.1.1.1:53"
                    .parse()
                    .map_err(|e| anyhow::anyhow!("parse upstream: {e}"))?],
                service_ttl_seconds: 5,
                upstream_timeout_ms: 2000,
                positive_cache_max_seconds: 300,
                negative_cache_max_seconds: 30,
                query_timeout_ms: 5000,
            },
            api: config::ApiConfig {
                listen: api_listen,
                admin_token_file: std::path::PathBuf::from("/nonexistent-use-inline"),
            },
            storage: config::StorageConfig {
                sqlite_path: sqlite_path.to_string(),
            },
            leases: config::LeaseConfig {
                heartbeat_interval_seconds: 20,
                agent_ttl_seconds: 90,
            },
            backup: config::BackupConfig {
                enabled: false,
                ..Default::default()
            },
            security: config::SecurityConfig::default(),
        };

        let storage = Storage::connect(sqlite_path, cfg.leases.agent_ttl_seconds)
            .await
            .context("storage")?;
        let registry = DnsRegistry::new();
        let mut state_inner = AppState::new(storage.clone(), registry.clone(), cfg.clone())?;
        state_inner.admin_token = admin_token.as_bytes().to_vec();
        state_inner
            .dns_up
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let state: SharedState = Arc::new(state_inner);

        refresh_dns_registry(&state).await?;

        let ttl = ChronoDuration::seconds(cfg.leases.agent_ttl_seconds as i64);
        let st_cleanup = state.clone();
        let cleanup_task = tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
            loop {
                interval.tick().await;
                let _ = st_cleanup.storage.purge_expired_agents(ttl).await;
                let _ = refresh_dns_registry(&st_cleanup).await;
            }
        });

        let dns_cfg = DnsServerConfig::with_defaults(
            dns_listen,
            cfg.dns.upstreams.clone(),
            cfg.dns.service_ttl_seconds,
            std::time::Duration::from_millis(cfg.dns.upstream_timeout_ms),
            registry.clone(),
        );
        let dns = DnsServer::new(dns_cfg).await?;
        let dns_task = tokio::spawn(async move {
            let _ = dns.run().await;
        });

        let app = api::router(state.clone());
        let listener = TcpListener::bind(api_listen).await?;
        let api_task = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        Ok(Self {
            state,
            api_listen,
            dns_listen,
            _api_task: api_task,
            _dns_task: dns_task,
            _cleanup_task: cleanup_task,
        })
    }

    pub fn api_base(&self) -> String {
        format!("http://{}", self.api_listen)
    }
}

pub async fn refresh_dns_registry(state: &AppState) -> anyhow::Result<()> {
    let ttl = ChronoDuration::seconds(state.cfg.leases.agent_ttl_seconds as i64);
    let routes = state.storage.active_dns_routes(ttl).await?;
    let mut map = std::collections::HashMap::new();
    for r in routes {
        if let Ok(ip) = r.tailscale_ipv4.parse::<Ipv4Addr>() {
            map.insert(r.hostname, ip);
        }
    }
    state.registry.replace(map);
    Ok(())
}

fn free_port() -> anyhow::Result<SocketAddr> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    Ok(SocketAddr::from(([127, 0, 0, 1], port)))
}
