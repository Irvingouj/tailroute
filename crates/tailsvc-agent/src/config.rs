use figment::{
    providers::{Env, Format, Toml},
    Figment,
};
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug, Clone, Deserialize)]
pub struct AgentConfig {
    pub controller_url: String,
    #[serde(default)]
    pub enrollment_token: Option<String>,
    #[serde(default = "default_docker_sock")]
    pub docker_socket: String,
    #[serde(default = "default_state")]
    pub state_dir: PathBuf,
    #[serde(default = "default_proxy_port")]
    pub proxy_port: u16,
    /// Override bind address (Compose / CI without Tailscale). Example: `0.0.0.0:8088`
    pub proxy_listen: Option<String>,
    /// Simulated Tailscale IPv4 for registration when Tailscale is absent (e.g. `100.64.0.2`).
    pub tailscale_ipv4: Option<String>,
    #[serde(default = "default_connect_ms")]
    pub connect_timeout_ms: u64,
    #[serde(default = "default_response_secs")]
    pub response_timeout_seconds: u64,
    /// If true, TCP-probe backends and log failures. Routes are still registered
    /// when the container is labeled and the backend URL resolves — probe must not
    /// drop hostnames on transient restart races (full-set PUT would delete DNS).
    #[serde(default = "default_probe")]
    pub tcp_probe_on_register: bool,
    /// Optional status HTTP bind for /health and /ready (SPEC §19). Example: `0.0.0.0:8089`
    pub status_listen: Option<String>,
    /// Non-Docker routes (e.g. controller admin UI, LAN services). Domain names live here only.
    #[serde(default)]
    pub static_routes: Vec<StaticRouteConfig>,
}

/// Config entry: one or more hostnames → one HTTP backend.
///
/// ```toml
/// [[static_routes]]
/// hosts = ["admin.example.com"]
/// backend = "http://127.0.0.1:18080"
/// ```
#[derive(Debug, Clone, Deserialize)]
pub struct StaticRouteConfig {
    /// Hostname list (already separate strings in TOML array).
    pub hosts: Vec<String>,
    /// `http://host:port` only (same rules as tailsvc.backend).
    pub backend: String,
}

fn default_docker_sock() -> String {
    "/var/run/docker.sock".into()
}
fn default_state() -> PathBuf {
    PathBuf::from("/var/lib/tailsvc-agent")
}
fn default_proxy_port() -> u16 {
    80
}
fn default_connect_ms() -> u64 {
    2000
}
fn default_response_secs() -> u64 {
    300
}
fn default_probe() -> bool {
    // Soft probe: warn only. Hard-skip was dropping routes after container restarts.
    true
}

impl AgentConfig {
    pub fn load() -> anyhow::Result<Self> {
        let path = std::env::var("TAILSVC_CONFIG").unwrap_or_else(|_| "config/agent.toml".into());
        let mut cfg: AgentConfig = Figment::new()
            .merge(Toml::file(&path))
            .merge(Env::prefixed("TAILSVC_").split("_"))
            .extract()
            .map_err(|e| anyhow::anyhow!("config: {e}"))?;
        if cfg.enrollment_token.is_none() {
            if let Ok(t) = std::env::var("TAILSVC_ENROLLMENT_TOKEN") {
                cfg.enrollment_token = Some(t);
            }
        }
        if let Ok(u) = std::env::var("TAILSVC_CONTROLLER") {
            cfg.controller_url = u;
        }
        Ok(cfg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_static_routes_toml() {
        let raw = r#"
controller_url = "http://127.0.0.1:18080"
[[static_routes]]
hosts = ["admin.example.com", "panel.example.com"]
backend = "http://127.0.0.1:18080"
"#;
        let cfg: AgentConfig = Figment::new().merge(Toml::string(raw)).extract().unwrap();
        assert_eq!(cfg.static_routes.len(), 1);
        assert_eq!(cfg.static_routes[0].hosts.len(), 2);
        assert_eq!(cfg.static_routes[0].backend, "http://127.0.0.1:18080");
    }
}
