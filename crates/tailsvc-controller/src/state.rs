use crate::config::ControllerConfig;
use std::sync::Arc;
use tailsvc_dns::DnsRegistry;
use tailsvc_storage::Storage;

pub struct AppState {
    pub storage: Storage,
    pub registry: DnsRegistry,
    pub cfg: ControllerConfig,
    pub admin_token: Vec<u8>,
    /// Set true once DNS listener is known to be up.
    pub dns_up: std::sync::atomic::AtomicBool,
}

impl AppState {
    pub fn new(
        storage: Storage,
        registry: DnsRegistry,
        cfg: ControllerConfig,
    ) -> anyhow::Result<Self> {
        let admin_token = load_admin_token(&cfg)?;
        Ok(Self {
            storage,
            registry,
            cfg,
            admin_token,
            dns_up: std::sync::atomic::AtomicBool::new(false),
        })
    }
}

fn load_admin_token(cfg: &ControllerConfig) -> anyhow::Result<Vec<u8>> {
    match std::fs::read(&cfg.api.admin_token_file) {
        Ok(bytes) => {
            let s = String::from_utf8_lossy(&bytes);
            let t = s.trim();
            if t.is_empty() {
                anyhow::bail!(
                    "admin token file {} is empty",
                    cfg.api.admin_token_file.display()
                );
            }
            Ok(t.as_bytes().to_vec())
        }
        Err(e) => {
            // Dev-only fallback compiled out in production builds without `dev-defaults`.
            #[cfg(feature = "dev-defaults")]
            {
                tracing::warn!(
                    error = %e,
                    path = %cfg.api.admin_token_file.display(),
                    "admin token file missing; using dev-admin-token (dev-defaults feature)"
                );
                Ok(b"dev-admin-token".to_vec())
            }
            #[cfg(not(feature = "dev-defaults"))]
            {
                let _ = e;
                anyhow::bail!(
                    "admin token file {} required (rebuild with --features dev-defaults for local dev fallback)",
                    cfg.api.admin_token_file.display()
                );
            }
        }
    }
}

pub type SharedState = Arc<AppState>;
