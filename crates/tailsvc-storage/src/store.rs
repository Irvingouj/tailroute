use chrono::{DateTime, Duration, Utc};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions};
use sqlx::Row;
use std::str::FromStr;
use tailsvc_common::api::{RouteConflict, RouteEntry};
use tailsvc_common::auth::{hash_token, verify_token};
use thiserror::Error;
use tracing::info;

#[derive(Debug, Error)]
pub enum StorageError {
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
    #[error(transparent)]
    Migrate(#[from] sqlx::migrate::MigrateError),
    #[error("not found")]
    NotFound,
    #[error("conflict")]
    Conflict,
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, StorageError>;

#[derive(Clone)]
pub struct Storage {
    pool: SqlitePool,
    agent_ttl: Duration,
    sqlite_path: String,
}

#[derive(Debug, Clone)]
pub struct AgentRecord {
    pub agent_id: String,
    pub display_name: String,
    pub tailscale_ipv4: String,
    pub docker_engine_id: String,
    pub created_at: DateTime<Utc>,
    pub last_seen_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct DnsRoute {
    pub hostname: String,
    pub tailscale_ipv4: String,
}

#[derive(Debug, Clone)]
pub struct RouteConflictRecord {
    pub hostname: String,
    pub owner_agent_id: String,
    pub lease_expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct PutRoutesOutcome {
    pub accepted: Vec<String>,
    pub conflicts: Vec<RouteConflict>,
}

#[derive(Debug, Clone)]
pub struct EnrollmentTokenRecord {
    pub token_plain: String,
}

impl Storage {
    pub async fn connect(sqlite_path: &str, agent_ttl_seconds: u64) -> Result<Self> {
        let opts = SqliteConnectOptions::from_str(&format!("sqlite:{sqlite_path}?mode=rwc"))?
            .create_if_missing(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(opts)
            .await?;
        sqlx::migrate!().run(&pool).await?;
        Ok(Self {
            pool,
            agent_ttl: Duration::seconds(agent_ttl_seconds as i64),
            sqlite_path: sqlite_path.to_string(),
        })
    }

    pub fn agent_ttl(&self) -> Duration {
        self.agent_ttl
    }

    pub fn sqlite_path(&self) -> &str {
        &self.sqlite_path
    }

    /// Consistent online backup via SQLite `VACUUM INTO`.
    pub async fn backup_to_dir(&self, dir: &std::path::Path) -> Result<std::path::PathBuf> {
        std::fs::create_dir_all(dir).map_err(|e| StorageError::Other(e.to_string()))?;
        let name = format!(
            "controller-{}.db",
            chrono::Utc::now().format("%Y%m%dT%H%M%SZ")
        );
        let dest = dir.join(&name);
        let dest_str = dest
            .to_str()
            .ok_or_else(|| StorageError::Other("backup path not utf-8".into()))?
            .replace('\'', "''");
        // VACUUM INTO creates a consistent snapshot without stopping writers.
        let sql = format!("VACUUM INTO '{dest_str}'");
        sqlx::query(&sql)
            .execute(&self.pool)
            .await
            .map_err(StorageError::Sqlx)?;
        Ok(dest)
    }

    pub async fn create_enrollment_token(&self) -> Result<EnrollmentTokenRecord> {
        let token = tailsvc_common::auth::generate_token(32);
        let token_hash = hash_token(&token).map_err(StorageError::Other)?;
        let now = Utc::now().to_rfc3339();
        sqlx::query("INSERT INTO enrollment_tokens (token_hash, created_at) VALUES (?, ?)")
            .bind(&token_hash)
            .bind(&now)
            .execute(&self.pool)
            .await?;
        Ok(EnrollmentTokenRecord { token_plain: token })
    }

    pub async fn consume_enrollment_token(&self, token: &str) -> Result<()> {
        let rows =
            sqlx::query("SELECT token_hash, used_at FROM enrollment_tokens WHERE used_at IS NULL")
                .fetch_all(&self.pool)
                .await?;
        for row in rows {
            let hash: String = row.get("token_hash");
            if verify_token(&hash, token) {
                let used = Utc::now().to_rfc3339();
                sqlx::query("UPDATE enrollment_tokens SET used_at = ? WHERE token_hash = ?")
                    .bind(&used)
                    .bind(&hash)
                    .execute(&self.pool)
                    .await?;
                return Ok(());
            }
        }
        Err(StorageError::Other("invalid enrollment token".into()))
    }

    pub async fn enroll_agent(
        &self,
        display_name: &str,
        tailscale_ipv4: &str,
        docker_engine_id: &str,
    ) -> Result<(String, String)> {
        let agent_id = format!("agt_{}", tailsvc_common::auth::generate_token(12));
        let agent_token = tailsvc_common::auth::generate_token(32);
        let credential_hash = hash_token(&agent_token).map_err(StorageError::Other)?;
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO agents (agent_id, display_name, tailscale_ipv4, docker_engine_id, credential_hash, created_at, last_seen_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&agent_id)
        .bind(display_name)
        .bind(tailscale_ipv4)
        .bind(docker_engine_id)
        .bind(&credential_hash)
        .bind(&now)
        .bind(&now)
        .execute(&self.pool)
        .await?;
        Ok((agent_id, agent_token))
    }

    pub async fn verify_agent_token(&self, agent_id: &str, token: &str) -> Result<()> {
        let row = sqlx::query("SELECT credential_hash, revoked_at FROM agents WHERE agent_id = ?")
            .bind(agent_id)
            .fetch_optional(&self.pool)
            .await?;
        let row = row.ok_or(StorageError::NotFound)?;
        if row.get::<Option<String>, _>("revoked_at").is_some() {
            return Err(StorageError::Other("revoked".into()));
        }
        let hash: String = row.get("credential_hash");
        if verify_token(&hash, token) {
            Ok(())
        } else {
            Err(StorageError::Other("bad token".into()))
        }
    }

    pub async fn heartbeat(&self, agent_id: &str, tailscale_ipv4: &str) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        let r = sqlx::query(
            "UPDATE agents SET last_seen_at = ?, tailscale_ipv4 = ? WHERE agent_id = ? AND revoked_at IS NULL",
        )
        .bind(&now)
        .bind(tailscale_ipv4)
        .bind(agent_id)
        .execute(&self.pool)
        .await?;
        if r.rows_affected() == 0 {
            return Err(StorageError::NotFound);
        }
        Ok(())
    }

    pub async fn put_routes(
        &self,
        agent_id: &str,
        routes: &[RouteEntry],
        ttl: Duration,
    ) -> Result<PutRoutesOutcome> {
        let agent = self.get_agent(agent_id).await?;
        let _last_seen = agent.last_seen_at.unwrap_or_else(Utc::now);

        let mut accepted = Vec::new();
        let mut conflicts = Vec::new();

        let desired: std::collections::HashSet<String> = routes
            .iter()
            .map(|r| r.hostname.to_ascii_lowercase())
            .collect();

        let mut tx = self.pool.begin().await?;

        for entry in routes {
            let host = entry.hostname.to_ascii_lowercase();
            let existing = sqlx::query(
                "SELECT r.hostname, r.agent_id, a.last_seen_at FROM routes r
                 JOIN agents a ON a.agent_id = r.agent_id
                 WHERE r.hostname = ?",
            )
            .bind(&host)
            .fetch_optional(&mut *tx)
            .await?;

            if let Some(row) = existing {
                let owner: String = row.get("agent_id");
                let owner_last: Option<String> = row.get("last_seen_at");
                let owner_alive = owner_last
                    .as_ref()
                    .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                    .map(|t| Utc::now() - t.with_timezone(&Utc) < ttl)
                    .unwrap_or(false);

                if owner != agent_id && owner_alive {
                    let ls = owner_last
                        .as_ref()
                        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                        .map(|t| t.with_timezone(&Utc))
                        .unwrap_or_else(Utc::now);
                    conflicts.push(RouteConflict {
                        hostname: host.clone(),
                        owner_agent_id: owner,
                        lease_expires_at: ls + ttl,
                    });
                    continue;
                }
            }

            let now = Utc::now().to_rfc3339();
            sqlx::query(
                "INSERT INTO routes (hostname, agent_id, backend, container_id, container_name, updated_at)
                 VALUES (?, ?, ?, ?, ?, ?)
                 ON CONFLICT(hostname) DO UPDATE SET
                   agent_id = excluded.agent_id,
                   backend = excluded.backend,
                   container_id = excluded.container_id,
                   container_name = excluded.container_name,
                   updated_at = excluded.updated_at",
            )
            .bind(&host)
            .bind(agent_id)
            .bind(&entry.backend)
            .bind(&entry.container_id)
            .bind(&entry.container_name)
            .bind(&now)
            .execute(&mut *tx)
            .await?;
            accepted.push(host);
        }

        let owned = sqlx::query("SELECT hostname FROM routes WHERE agent_id = ?")
            .bind(agent_id)
            .fetch_all(&mut *tx)
            .await?;
        for row in owned {
            let h: String = row.get("hostname");
            if !desired.contains(&h) {
                sqlx::query("DELETE FROM routes WHERE hostname = ? AND agent_id = ?")
                    .bind(&h)
                    .bind(agent_id)
                    .execute(&mut *tx)
                    .await?;
            }
        }

        tx.commit().await?;
        Ok(PutRoutesOutcome {
            accepted,
            conflicts,
        })
    }

    pub async fn get_agent(&self, agent_id: &str) -> Result<AgentRecord> {
        let row = sqlx::query(
            "SELECT agent_id, display_name, tailscale_ipv4, docker_engine_id, created_at, last_seen_at
             FROM agents WHERE agent_id = ? AND revoked_at IS NULL",
        )
        .bind(agent_id)
        .fetch_optional(&self.pool)
        .await?;
        let row = row.ok_or(StorageError::NotFound)?;
        Ok(row_to_agent(&row))
    }

    pub async fn list_agents(&self) -> Result<Vec<AgentRecord>> {
        let rows = sqlx::query(
            "SELECT agent_id, display_name, tailscale_ipv4, docker_engine_id, created_at, last_seen_at
             FROM agents WHERE revoked_at IS NULL",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.iter().map(row_to_agent).collect())
    }

    pub async fn list_routes_admin(
        &self,
        ttl: Duration,
    ) -> Result<
        Vec<(
            String,
            String,
            String,
            Option<String>,
            Option<String>,
            DateTime<Utc>,
        )>,
    > {
        let rows = sqlx::query(
            "SELECT r.hostname, r.agent_id, r.backend, r.container_id, r.container_name, a.tailscale_ipv4, a.last_seen_at
             FROM routes r JOIN agents a ON a.agent_id = r.agent_id WHERE a.revoked_at IS NULL",
        )
        .fetch_all(&self.pool)
        .await?;
        let mut out = Vec::new();
        let now = Utc::now();
        for row in rows {
            let last: Option<String> = row.get("last_seen_at");
            let alive = last
                .as_ref()
                .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                .map(|t| now - t.with_timezone(&Utc) < ttl)
                .unwrap_or(false);
            if !alive {
                continue;
            }
            let ls = last
                .as_ref()
                .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                .map(|t| t.with_timezone(&Utc))
                .unwrap_or(now);
            out.push((
                row.get("hostname"),
                row.get("agent_id"),
                row.get("backend"),
                row.get("container_id"),
                row.get("container_name"),
                ls + ttl,
            ));
        }
        Ok(out)
    }

    pub async fn delete_route(&self, hostname: &str) -> Result<()> {
        sqlx::query("DELETE FROM routes WHERE hostname = ?")
            .bind(hostname.to_ascii_lowercase())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn revoke_agent(&self, agent_id: &str) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        sqlx::query("UPDATE agents SET revoked_at = ? WHERE agent_id = ?")
            .bind(&now)
            .bind(agent_id)
            .execute(&self.pool)
            .await?;
        sqlx::query("DELETE FROM routes WHERE agent_id = ?")
            .bind(agent_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn active_dns_routes(&self, ttl: Duration) -> Result<Vec<DnsRoute>> {
        let rows = sqlx::query(
            "SELECT r.hostname, a.tailscale_ipv4, a.last_seen_at
             FROM routes r JOIN agents a ON a.agent_id = r.agent_id
             WHERE a.revoked_at IS NULL",
        )
        .fetch_all(&self.pool)
        .await?;
        let now = Utc::now();
        let mut out = Vec::new();
        for row in rows {
            let last: Option<String> = row.get("last_seen_at");
            let alive = last
                .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                .map(|t| now - t.with_timezone(&Utc) < ttl)
                .unwrap_or(false);
            if alive {
                out.push(DnsRoute {
                    hostname: row.get("hostname"),
                    tailscale_ipv4: row.get("tailscale_ipv4"),
                });
            }
        }
        Ok(out)
    }

    pub async fn purge_expired_agents(&self, ttl: Duration) -> Result<u64> {
        let cutoff = (Utc::now() - ttl).to_rfc3339();
        let r = sqlx::query(
            "DELETE FROM routes WHERE agent_id IN (
                SELECT agent_id FROM agents WHERE last_seen_at < ? OR last_seen_at IS NULL
             )",
        )
        .bind(&cutoff)
        .execute(&self.pool)
        .await?;
        info!(deleted_routes = r.rows_affected(), "purged stale routes");
        Ok(r.rows_affected())
    }
}

fn row_to_agent(row: &sqlx::sqlite::SqliteRow) -> AgentRecord {
    let created: String = row.get("created_at");
    let last: Option<String> = row.get("last_seen_at");
    AgentRecord {
        agent_id: row.get("agent_id"),
        display_name: row.get("display_name"),
        tailscale_ipv4: row.get("tailscale_ipv4"),
        docker_engine_id: row.get("docker_engine_id"),
        created_at: DateTime::parse_from_rfc3339(&created)
            .map(|t| t.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now()),
        last_seen_at: last.and_then(|s| {
            DateTime::parse_from_rfc3339(&s)
                .ok()
                .map(|t| t.with_timezone(&Utc))
        }),
    }
}
