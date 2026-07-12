use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnrollRequest {
    pub display_name: String,
    pub tailscale_ipv4: String,
    pub docker_engine_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnrollResponse {
    pub agent_id: String,
    pub agent_token: String,
    pub heartbeat_interval_seconds: u64,
    pub lease_ttl_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeartbeatRouteRef {
    pub hostname: String,
    pub backend_fingerprint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeartbeatRequest {
    pub tailscale_ipv4: String,
    pub routes: Vec<HeartbeatRouteRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteEntry {
    pub hostname: String,
    pub backend: String,
    pub container_id: Option<String>,
    pub container_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PutRoutesRequest {
    pub routes: Vec<RouteEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteConflict {
    pub hostname: String,
    pub owner_agent_id: String,
    pub lease_expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PutRoutesResponse {
    pub accepted: Vec<String>,
    pub conflicts: Vec<RouteConflict>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminAgentView {
    pub agent_id: String,
    pub display_name: String,
    pub tailscale_ipv4: String,
    pub docker_engine_id: String,
    pub created_at: DateTime<Utc>,
    pub last_seen_at: Option<DateTime<Utc>>,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminRouteView {
    pub hostname: String,
    pub agent_id: String,
    pub tailscale_ipv4: String,
    pub backend: String,
    pub container_id: Option<String>,
    pub container_name: Option<String>,
    pub lease_expires_at: DateTime<Utc>,
}

/// Dashboard row: registration lease + optional HTTP probe via agent data path.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminRouteHealthView {
    pub hostname: String,
    pub agent_id: String,
    pub tailscale_ipv4: String,
    pub backend: String,
    pub container_id: Option<String>,
    pub container_name: Option<String>,
    pub lease_expires_at: DateTime<Utc>,
    /// `healthy` if lease not expired; else `stale`.
    pub registration: String,
    /// `ok` | `fail` | `timeout` | `skipped`
    pub http_probe: String,
    pub probe_ms: Option<u64>,
    pub probe_detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminDashboard {
    pub routes: Vec<AdminRouteHealthView>,
    pub agents: Vec<AdminAgentView>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateEnrollmentTokenResponse {
    pub token: String,
    pub expires_at: Option<DateTime<Utc>>,
}
