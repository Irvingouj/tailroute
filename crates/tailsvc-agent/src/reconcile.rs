use crate::agent_state::{self, PersistedAgentState};
use crate::config::AgentConfig;
use crate::controller_client::ControllerClient;
use crate::ready::AgentReady;
use crate::static_routes::{apply_static_routes, prepare_static_routes, PreparedStaticRoute};
use futures::StreamExt;
use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;
use tailsvc_common::api::{
    EnrollRequest, HeartbeatRequest, HeartbeatRouteRef, PutRoutesRequest, RouteEntry,
};
use tailsvc_docker::{backend_url, is_relevant_event, resolve_backend, DockerRuntime};
use tailsvc_proxy::{BackendRoute, SharedRouteStore};
use tracing::{info, warn};

const RECONCILE_DEBOUNCE: Duration = Duration::from_millis(400);
/// Periodic full reconcile so routes recover after start races / missed events.
const PERIODIC_RECONCILE: Duration = Duration::from_secs(15);
const EVENT_RECONNECT_BASE: Duration = Duration::from_secs(1);
const EVENT_RECONNECT_MAX: Duration = Duration::from_secs(30);
const CONTROLLER_BACKOFF_BASE: Duration = Duration::from_millis(500);
const CONTROLLER_BACKOFF_MAX: Duration = Duration::from_secs(30);
const PROBE_RETRIES: u32 = 5;
const PROBE_RETRY_DELAY: Duration = Duration::from_millis(400);

pub struct AgentRun {
    cfg: AgentConfig,
    routes: Arc<SharedRouteStore>,
    tailscale_ip: Ipv4Addr,
    docker: DockerRuntime,
    client: ControllerClient,
    heartbeat_interval: Duration,
    controller_backoff: Duration,
    ready: AgentReady,
}

impl AgentRun {
    pub async fn new(
        cfg: AgentConfig,
        routes: Arc<SharedRouteStore>,
        tailscale_ip: Ipv4Addr,
        ready: AgentReady,
    ) -> anyhow::Result<Self> {
        std::fs::create_dir_all(&cfg.state_dir)?;
        let docker = DockerRuntime::connect(&cfg.docker_socket)?;
        ready.set(&ready.docker_ok, true);

        let persisted = agent_state::load(&cfg.state_dir)?;
        let (agent_id, token, heartbeat_interval) = if let Some(p) = persisted {
            if p.controller_url != cfg.controller_url {
                warn!("controller url changed; re-enroll may be required");
            }
            ready.set(&ready.enrolled, true);
            (p.agent_id, p.agent_token, Duration::from_secs(20))
        } else {
            let enroll_token = cfg.enrollment_token.clone().ok_or_else(|| {
                anyhow::anyhow!("TAILSVC_ENROLLMENT_TOKEN required for first run")
            })?;
            let engine_id = docker_engine_id(&docker)
                .await
                .unwrap_or_else(|| "unknown".into());
            let resp = ControllerClient::enroll(
                &cfg.controller_url,
                &enroll_token,
                EnrollRequest {
                    display_name: hostname::get()
                        .ok()
                        .and_then(|h| h.into_string().ok())
                        .unwrap_or_else(|| "docker-host".into()),
                    tailscale_ipv4: tailscale_ip.to_string(),
                    docker_engine_id: engine_id,
                },
            )
            .await?;
            agent_state::save(
                &cfg.state_dir,
                &PersistedAgentState {
                    agent_id: resp.agent_id.clone(),
                    agent_token: resp.agent_token.clone(),
                    controller_url: cfg.controller_url.clone(),
                },
            )?;
            ready.set(&ready.enrolled, true);
            (
                resp.agent_id,
                resp.agent_token,
                Duration::from_secs(resp.heartbeat_interval_seconds),
            )
        };

        let client = ControllerClient::new(cfg.controller_url.clone(), agent_id, token);
        Ok(Self {
            cfg,
            routes,
            tailscale_ip,
            docker,
            client,
            heartbeat_interval,
            controller_backoff: CONTROLLER_BACKOFF_BASE,
            ready,
        })
    }

    pub async fn loop_forever(&mut self) -> anyhow::Result<()> {
        // Initial full scan.
        self.reconcile_with_backoff().await;
        self.ready.set(&self.ready.initial_reconcile, true);

        let mut hb = tokio::time::interval(self.heartbeat_interval);
        let mut periodic = tokio::time::interval(PERIODIC_RECONCILE);
        periodic.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut reconnect_delay = EVENT_RECONNECT_BASE;

        loop {
            let mut events = self.docker.watch_events();
            let mut debounce: Option<tokio::time::Instant> = None;
            let mut saw_event = false;
            info!("docker event stream connected");

            loop {
                tokio::select! {
                    ev = events.next() => {
                        match ev {
                            None => {
                                warn!("docker event stream ended; will reconnect and rescan");
                                break;
                            }
                            Some(Err(e)) => {
                                warn!(error = %e, "docker event error; will reconnect and rescan");
                                break;
                            }
                            Some(Ok(msg)) => {
                                saw_event = true;
                                if is_relevant_event(&msg) {
                                    debounce = Some(tokio::time::Instant::now() + RECONCILE_DEBOUNCE);
                                }
                            }
                        }
                    }
                    _ = async {
                        if let Some(deadline) = debounce {
                            tokio::time::sleep_until(deadline).await;
                        } else {
                            std::future::pending::<()>().await;
                        }
                    } => {
                        debounce = None;
                        self.reconcile_with_backoff().await;
                    }
                    _ = periodic.tick() => {
                        // Recover routes dropped by start-race probe failures / missed events.
                        self.reconcile_with_backoff().await;
                    }
                    _ = hb.tick() => {
                        if let Err(e) = self.heartbeat_only().await {
                            warn!(error = %e, "heartbeat failed");
                            self.bump_controller_backoff();
                        } else {
                            self.controller_backoff = CONTROLLER_BACKOFF_BASE;
                        }
                    }
                }
            }

            // Stream interrupted: full rescan then reconnect with backoff.
            if saw_event {
                reconnect_delay = EVENT_RECONNECT_BASE;
            }
            self.reconcile_with_backoff().await;
            tokio::time::sleep(reconnect_delay).await;
            reconnect_delay = (reconnect_delay * 2).min(EVENT_RECONNECT_MAX);
        }
    }

    async fn reconcile_with_backoff(&mut self) {
        match self.reconcile_once().await {
            Ok(()) => {
                self.controller_backoff = CONTROLLER_BACKOFF_BASE;
            }
            Err(e) => {
                warn!(
                    error = %e,
                    backoff_ms = self.controller_backoff.as_millis(),
                    "reconcile failed"
                );
                tokio::time::sleep(self.controller_backoff).await;
                self.bump_controller_backoff();
                // Keep local proxy routes; only controller sync failed.
                if let Err(e2) = self.refresh_local_routes().await {
                    warn!(error = %e2, "local route refresh failed");
                }
            }
        }
    }

    fn bump_controller_backoff(&mut self) {
        self.controller_backoff = (self.controller_backoff * 2).min(CONTROLLER_BACKOFF_MAX);
    }

    async fn refresh_local_routes(&self) -> anyhow::Result<()> {
        let services = self.docker.list_services().await?;
        let mut local = HashMap::new();
        for svc in services {
            let backend = match resolve_backend(&svc) {
                Ok(b) => b,
                Err(e) => {
                    warn!(container = %svc.container_name, error = %e, "skip container");
                    continue;
                }
            };
            if self.cfg.tcp_probe_on_register && !tcp_probe_with_retries(&backend).await {
                // Soft fail: still register. Hard skip + full-set PUT removed DNS on restarts.
                warn!(
                    container = %svc.container_name,
                    "tcp probe failed (still registering)"
                );
            }
            for host in &svc.labels.hosts {
                // Public-domain shadowing warning (SPEC §4.2 / §18.5).
                if looks_like_public_domain(host.as_str()) {
                    warn!(
                        hostname = %host.as_str(),
                        container = %svc.container_name,
                        "registering hostname that may shadow a public domain"
                    );
                }
                local.insert(
                    host.as_str().to_string(),
                    BackendRoute {
                        backend: backend.clone(),
                        container_id: Some(svc.container_id.clone()),
                        container_name: Some(svc.container_name.clone()),
                    },
                );
            }
        }
        let mut api_routes = Vec::new();
        let mut hb_refs = Vec::new();
        let prepared = self.prepared_static_routes().await;
        let _ = apply_static_routes(&mut local, &mut api_routes, &mut hb_refs, &prepared);
        let _ = (api_routes, hb_refs);
        self.routes.replace(local);
        Ok(())
    }

    async fn prepared_static_routes(&self) -> Vec<PreparedStaticRoute> {
        let prepared = prepare_static_routes(&self.cfg.static_routes);
        if self.cfg.tcp_probe_on_register {
            for sr in &prepared {
                if !tcp_probe_with_retries(&sr.backend).await {
                    warn!(
                        backend = %sr.backend_url,
                        "static_routes: tcp probe failed (still registering)"
                    );
                }
            }
        }
        prepared
    }

    async fn reconcile_once(&mut self) -> anyhow::Result<()> {
        let services = self.docker.list_services().await?;
        let mut local = HashMap::new();
        let mut api_routes = Vec::new();
        let mut hb_refs = Vec::new();

        for svc in services {
            let backend = match resolve_backend(&svc) {
                Ok(b) => b,
                Err(e) => {
                    warn!(container = %svc.container_name, error = %e, "skip container");
                    continue;
                }
            };
            if self.cfg.tcp_probe_on_register && !tcp_probe_with_retries(&backend).await {
                warn!(
                    container = %svc.container_name,
                    "tcp probe failed (still registering)"
                );
            }
            let url = backend_url(&backend);
            for host in &svc.labels.hosts {
                if looks_like_public_domain(host.as_str()) {
                    warn!(
                        hostname = %host.as_str(),
                        container = %svc.container_name,
                        "registering hostname that may shadow a public domain"
                    );
                }
                let key = host.as_str().to_string();
                local.insert(
                    key.clone(),
                    BackendRoute {
                        backend: backend.clone(),
                        container_id: Some(svc.container_id.clone()),
                        container_name: Some(svc.container_name.clone()),
                    },
                );
                api_routes.push(RouteEntry {
                    hostname: key.clone(),
                    backend: url.clone(),
                    container_id: Some(svc.container_id.clone()),
                    container_name: Some(svc.container_name.clone()),
                });
                hb_refs.push(HeartbeatRouteRef {
                    hostname: key,
                    backend_fingerprint: backend.fingerprint(),
                });
            }
        }

        let prepared = self.prepared_static_routes().await;
        let (static_n, _) =
            apply_static_routes(&mut local, &mut api_routes, &mut hb_refs, &prepared);
        if static_n > 0 {
            info!(static_hosts = static_n, "static_routes applied");
        }

        // Update local proxy first so controller outages do not stop serving.
        self.routes.replace(local);

        let put = PutRoutesRequest {
            routes: api_routes.clone(),
        };
        let resp = self.client.put_routes(put).await?;
        for c in &resp.conflicts {
            warn!(
                hostname = %c.hostname,
                owner = %c.owner_agent_id,
                "hostname conflict"
            );
        }
        info!(accepted = resp.accepted.len(), "routes synced");
        let _ = self
            .client
            .heartbeat(HeartbeatRequest {
                tailscale_ipv4: self.tailscale_ip.to_string(),
                routes: hb_refs,
            })
            .await;
        Ok(())
    }

    async fn heartbeat_only(&self) -> anyhow::Result<()> {
        self.client
            .heartbeat(HeartbeatRequest {
                tailscale_ipv4: self.tailscale_ip.to_string(),
                routes: vec![],
            })
            .await
    }
}

fn looks_like_public_domain(host: &str) -> bool {
    // Heuristic: has a dot and does not end with .internal / .local / .localhost / .test / .lan
    if !host.contains('.') {
        return false;
    }
    let lower = host.to_ascii_lowercase();
    !(lower.ends_with(".internal")
        || lower.ends_with(".local")
        || lower.ends_with(".localhost")
        || lower.ends_with(".test")
        || lower.ends_with(".lan")
        || lower.ends_with(".home")
        || lower.ends_with(".corp"))
}

async fn tcp_probe_with_retries(backend: &tailsvc_common::backend::Backend) -> bool {
    for attempt in 0..PROBE_RETRIES {
        if tcp_probe(backend).await {
            return true;
        }
        if attempt + 1 < PROBE_RETRIES {
            tokio::time::sleep(PROBE_RETRY_DELAY).await;
        }
    }
    false
}

async fn tcp_probe(backend: &tailsvc_common::backend::Backend) -> bool {
    let addr: SocketAddr = match format!("{}:{}", backend.host, backend.port).parse() {
        Ok(a) => a,
        Err(_) => {
            // hostname backend — try lookup
            match tokio::net::lookup_host(format!("{}:{}", backend.host, backend.port)).await {
                Ok(mut it) => match it.next() {
                    Some(a) => a,
                    None => return false,
                },
                Err(_) => return false,
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(2), tokio::net::TcpStream::connect(addr))
        .await
        .ok()
        .and_then(|r| r.ok())
        .is_some()
}

async fn docker_engine_id(docker: &DockerRuntime) -> Option<String> {
    let info = docker.docker().info().await.ok()?;
    info.id
}
