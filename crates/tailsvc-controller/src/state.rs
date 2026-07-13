use crate::config::ControllerConfig;
use crate::sessions::SessionStore;
use std::sync::Arc;
use std::time::Duration;
use tailsvc_common::auth::constant_time_eq;
use tailsvc_dns::DnsRegistry;
use tailsvc_storage::Storage;

pub struct AppState {
    pub storage: Storage,
    pub registry: DnsRegistry,
    pub cfg: ControllerConfig,
    /// Admin password bytes (from admin_password_file / admin_token_file).
    pub admin_password: Vec<u8>,
    pub admin_username: String,
    pub sessions: SessionStore,
    /// Set true once DNS listener is known to be up.
    pub dns_up: std::sync::atomic::AtomicBool,
}

impl AppState {
    pub fn new(
        storage: Storage,
        registry: DnsRegistry,
        cfg: ControllerConfig,
    ) -> anyhow::Result<Self> {
        let admin_password = load_admin_password(&cfg)?;
        let admin_username = cfg.api.admin_username.clone();
        let ttl = Duration::from_secs(cfg.api.session_ttl_seconds.max(300));
        Ok(Self {
            storage,
            registry,
            cfg,
            admin_password,
            admin_username,
            sessions: SessionStore::new(ttl),
            dns_up: std::sync::atomic::AtomicBool::new(false),
        })
    }

    /// Accept session token from login, or password as Bearer (CLI/scripts).
    pub fn admin_authorized(&self, bearer: &str) -> bool {
        if self.sessions.valid(bearer) {
            return true;
        }
        constant_time_eq(self.admin_password.as_slice(), bearer.as_bytes())
    }

    pub fn verify_password(&self, username: &str, password: &str) -> bool {
        if !constant_time_eq(self.admin_username.as_bytes(), username.as_bytes()) {
            // Still compare password to avoid trivial username oracle timing (best-effort).
            let _ = constant_time_eq(self.admin_password.as_slice(), password.as_bytes());
            return false;
        }
        constant_time_eq(self.admin_password.as_slice(), password.as_bytes())
    }
}

fn load_admin_password(cfg: &ControllerConfig) -> anyhow::Result<Vec<u8>> {
    let path = cfg
        .api
        .admin_password_file
        .as_ref()
        .unwrap_or(&cfg.api.admin_token_file);
    match std::fs::read(path) {
        Ok(bytes) => {
            let s = String::from_utf8_lossy(&bytes);
            let t = s.trim();
            if t.is_empty() {
                anyhow::bail!("admin password file {} is empty", path.display());
            }
            Ok(t.as_bytes().to_vec())
        }
        Err(e) => {
            #[cfg(feature = "dev-defaults")]
            {
                tracing::warn!(
                    error = %e,
                    path = %path.display(),
                    "admin password file missing; using dev-admin / dev-admin-token (dev-defaults)"
                );
                Ok(b"dev-admin-token".to_vec())
            }
            #[cfg(not(feature = "dev-defaults"))]
            {
                let _ = e;
                anyhow::bail!(
                    "admin password file {} required (or admin_token_file for compatibility)",
                    path.display()
                );
            }
        }
    }
}

pub type SharedState = Arc<AppState>;
