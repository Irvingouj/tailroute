use figment::{
    providers::{Env, Format, Toml},
    Figment,
};
use serde::Deserialize;
use std::net::SocketAddr;
use std::path::PathBuf;

#[derive(Debug, Clone, Deserialize)]
pub struct ControllerConfig {
    pub dns: DnsConfig,
    pub api: ApiConfig,
    pub storage: StorageConfig,
    pub leases: LeaseConfig,
    #[serde(default)]
    pub backup: BackupConfig,
    #[serde(default)]
    pub security: SecurityConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DnsConfig {
    pub listen: SocketAddr,
    pub upstreams: Vec<SocketAddr>,
    #[serde(default = "default_service_ttl")]
    pub service_ttl_seconds: u32,
    #[serde(default = "default_upstream_timeout")]
    pub upstream_timeout_ms: u64,
    #[serde(default = "default_positive_cache")]
    pub positive_cache_max_seconds: u64,
    #[serde(default = "default_negative_cache")]
    pub negative_cache_max_seconds: u64,
    #[serde(default = "default_query_timeout")]
    pub query_timeout_ms: u64,
}

fn default_service_ttl() -> u32 {
    5
}
fn default_upstream_timeout() -> u64 {
    2000
}
fn default_positive_cache() -> u64 {
    300
}
fn default_negative_cache() -> u64 {
    30
}
fn default_query_timeout() -> u64 {
    5000
}

#[derive(Debug, Clone, Deserialize)]
pub struct ApiConfig {
    pub listen: SocketAddr,
    pub admin_token_file: PathBuf,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StorageConfig {
    pub sqlite_path: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LeaseConfig {
    #[serde(default = "default_hb")]
    pub heartbeat_interval_seconds: u64,
    #[serde(default = "default_ttl")]
    pub agent_ttl_seconds: u64,
}

fn default_hb() -> u64 {
    20
}
fn default_ttl() -> u64 {
    90
}

#[derive(Debug, Clone, Deserialize)]
pub struct BackupConfig {
    #[serde(default = "default_backup_enabled")]
    pub enabled: bool,
    #[serde(default = "default_backup_dir")]
    pub dir: PathBuf,
    #[serde(default = "default_backup_interval")]
    pub interval_seconds: u64,
    #[serde(default = "default_backup_keep")]
    pub keep: usize,
}

impl Default for BackupConfig {
    fn default() -> Self {
        Self {
            enabled: default_backup_enabled(),
            dir: default_backup_dir(),
            interval_seconds: default_backup_interval(),
            keep: default_backup_keep(),
        }
    }
}

fn default_backup_enabled() -> bool {
    true
}
fn default_backup_dir() -> PathBuf {
    PathBuf::from("/var/lib/tailsvc-controller/backups")
}
fn default_backup_interval() -> u64 {
    3600
}
fn default_backup_keep() -> usize {
    48
}

/// Optional registration policy (safer global DNS).
#[derive(Debug, Clone, Deserialize, Default)]
pub struct SecurityConfig {
    /// If non-empty, only hostnames ending with these suffixes may be registered.
    #[serde(default)]
    pub allowed_suffixes: Vec<String>,
    /// Exact hostnames that must never be registered (e.g. github.com).
    #[serde(default)]
    pub blocked_names: Vec<String>,
}

impl ControllerConfig {
    pub fn load() -> anyhow::Result<Self> {
        let path =
            std::env::var("TAILSVC_CONFIG").unwrap_or_else(|_| "config/controller.toml".into());
        let cfg: ControllerConfig = Figment::new()
            .merge(Toml::file(&path))
            .merge(Env::prefixed("TAILSVC_").split("_"))
            .extract()
            .map_err(|e| anyhow::anyhow!("config: {e}"))?;
        Ok(cfg)
    }

    pub fn hostname_allowed(&self, host: &str) -> Result<(), String> {
        let h = host.to_ascii_lowercase();
        for b in &self.security.blocked_names {
            if h == b.to_ascii_lowercase() {
                return Err(format!("hostname blocked by policy: {host}"));
            }
        }
        if self.security.allowed_suffixes.is_empty() {
            return Ok(());
        }
        for suf in &self.security.allowed_suffixes {
            let s = suf.to_ascii_lowercase();
            if h == s.trim_start_matches('.') || h.ends_with(&s) {
                return Ok(());
            }
        }
        Err(format!("hostname not under allowed_suffixes: {host}"))
    }
}
