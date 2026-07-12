use crate::state::{AppState, SharedState};
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use chrono::Duration as ChronoDuration;
use chrono::Utc;
use tailsvc_common::api::{
    AdminAgentView, AdminDashboard, AdminRouteHealthView, AdminRouteView,
    CreateEnrollmentTokenResponse, EnrollRequest, EnrollResponse, HeartbeatRequest,
    PutRoutesRequest, PutRoutesResponse,
};
use tailsvc_common::auth::constant_time_eq;
use tower_http::catch_panic::CatchPanicLayer;
use tower_http::trace::TraceLayer;

const ADMIN_HTML: &str = include_str!("../static/admin.html");

pub fn router(state: SharedState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/ready", get(ready))
        .route("/", get(root))
        .route("/admin", get(admin_ui))
        .route("/admin/", get(admin_ui))
        .route("/v1/agents/enroll", post(enroll))
        .route("/v1/agents/{agent_id}/heartbeat", post(heartbeat))
        .route("/v1/agents/{agent_id}/routes", put(put_routes))
        .route("/v1/admin/agents", get(admin_agents))
        .route("/v1/admin/routes", get(admin_routes))
        .route("/v1/admin/dashboard", get(admin_dashboard))
        .route("/v1/admin/routes/{hostname}", delete(admin_delete_route))
        .route(
            "/v1/admin/enrollment-tokens",
            post(admin_create_enrollment_token),
        )
        .route("/v1/admin/agents/{agent_id}", delete(admin_revoke_agent))
        // Per-request panic isolation — process stays up.
        .layer(CatchPanicLayer::new())
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

async fn root(headers: HeaderMap) -> impl IntoResponse {
    // Browsers hitting the controller (directly or via Agent Host routing) with HTML Accept.
    let accept = headers
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if accept.contains("text/html") {
        return (
            StatusCode::TEMPORARY_REDIRECT,
            [(header::LOCATION, "/admin/")],
            "",
        )
            .into_response();
    }
    (StatusCode::OK, "tailsvc-controller\n").into_response()
}

async fn admin_ui() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        ADMIN_HTML,
    )
}

async fn health() -> &'static str {
    "ok"
}

async fn ready(State(st): State<SharedState>) -> Result<&'static str, StatusCode> {
    st.storage
        .list_agents()
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    if !st.dns_up.load(std::sync::atomic::Ordering::Relaxed) {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }
    Ok("ready")
}

fn bearer_token(headers: &HeaderMap) -> Option<String> {
    let v = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    v.strip_prefix("Bearer ").map(|s| s.to_string())
}

async fn enroll(
    State(st): State<SharedState>,
    headers: HeaderMap,
    Json(body): Json<EnrollRequest>,
) -> Result<Json<EnrollResponse>, ApiError> {
    let token = bearer_token(&headers).ok_or(ApiError::Unauthorized)?;
    st.storage
        .consume_enrollment_token(&token)
        .await
        .map_err(|_| ApiError::Unauthorized)?;
    let (agent_id, agent_token) = st
        .storage
        .enroll_agent(
            &body.display_name,
            &body.tailscale_ipv4,
            &body.docker_engine_id,
        )
        .await
        .map_err(ApiError::Storage)?;
    Ok(Json(EnrollResponse {
        agent_id,
        agent_token,
        heartbeat_interval_seconds: st.cfg.leases.heartbeat_interval_seconds,
        lease_ttl_seconds: st.cfg.leases.agent_ttl_seconds,
    }))
}

async fn heartbeat(
    State(st): State<SharedState>,
    Path(agent_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<HeartbeatRequest>,
) -> Result<StatusCode, ApiError> {
    let token = bearer_token(&headers).ok_or(ApiError::Unauthorized)?;
    st.storage
        .verify_agent_token(&agent_id, &token)
        .await
        .map_err(|_| ApiError::Unauthorized)?;
    st.storage
        .heartbeat(&agent_id, &body.tailscale_ipv4)
        .await
        .map_err(ApiError::Storage)?;
    refresh_dns(&st).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn put_routes(
    State(st): State<SharedState>,
    Path(agent_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<PutRoutesRequest>,
) -> Result<Json<PutRoutesResponse>, ApiError> {
    let token = bearer_token(&headers).ok_or(ApiError::Unauthorized)?;
    st.storage
        .verify_agent_token(&agent_id, &token)
        .await
        .map_err(|_| ApiError::Unauthorized)?;
    // Security policy: allowed_suffixes / blocked_names
    for r in &body.routes {
        if let Err(msg) = st.cfg.hostname_allowed(&r.hostname) {
            return Err(ApiError::Forbidden(msg));
        }
    }
    let ttl = ChronoDuration::seconds(st.cfg.leases.agent_ttl_seconds as i64);
    let out = st
        .storage
        .put_routes(&agent_id, &body.routes, ttl)
        .await
        .map_err(ApiError::Storage)?;
    refresh_dns(&st).await?;
    Ok(Json(PutRoutesResponse {
        accepted: out.accepted,
        conflicts: out.conflicts,
    }))
}

fn check_admin(st: &AppState, headers: &HeaderMap) -> Result<(), ApiError> {
    let token = bearer_token(headers).ok_or(ApiError::Unauthorized)?;
    if constant_time_eq(st.admin_token.as_slice(), token.as_bytes()) {
        Ok(())
    } else {
        Err(ApiError::Unauthorized)
    }
}

async fn admin_agents(
    State(st): State<SharedState>,
    headers: HeaderMap,
) -> Result<Json<Vec<AdminAgentView>>, ApiError> {
    check_admin(&st, &headers)?;
    let agents = st.storage.list_agents().await.map_err(ApiError::Storage)?;
    let ttl = ChronoDuration::seconds(st.cfg.leases.agent_ttl_seconds as i64);
    let now = Utc::now();
    let views: Vec<_> = agents
        .into_iter()
        .map(|a| {
            let status = match a.last_seen_at {
                Some(ls) if now - ls < ttl => "healthy",
                _ => "stale",
            };
            AdminAgentView {
                agent_id: a.agent_id,
                display_name: a.display_name,
                tailscale_ipv4: a.tailscale_ipv4,
                docker_engine_id: a.docker_engine_id,
                created_at: a.created_at,
                last_seen_at: a.last_seen_at,
                status: status.into(),
            }
        })
        .collect();
    Ok(Json(views))
}

async fn admin_routes(
    State(st): State<SharedState>,
    headers: HeaderMap,
) -> Result<Json<Vec<AdminRouteView>>, ApiError> {
    check_admin(&st, &headers)?;
    let ttl = ChronoDuration::seconds(st.cfg.leases.agent_ttl_seconds as i64);
    let rows = st
        .storage
        .list_routes_admin(ttl)
        .await
        .map_err(ApiError::Storage)?;
    let agents = st.storage.list_agents().await.map_err(ApiError::Storage)?;
    let ip_by_agent: std::collections::HashMap<_, _> = agents
        .into_iter()
        .map(|a| (a.agent_id, a.tailscale_ipv4))
        .collect();
    let views: Vec<_> = rows
        .into_iter()
        .map(
            |(hostname, agent_id, backend, cid, cname, lease)| AdminRouteView {
                hostname,
                agent_id: agent_id.clone(),
                tailscale_ipv4: ip_by_agent.get(&agent_id).cloned().unwrap_or_default(),
                backend,
                container_id: cid,
                container_name: cname,
                lease_expires_at: lease,
            },
        )
        .collect();
    Ok(Json(views))
}

/// Routes + agents + semi HTTP probe via each agent TS IP:80 Host header.
async fn admin_dashboard(
    State(st): State<SharedState>,
    headers: HeaderMap,
) -> Result<Json<AdminDashboard>, ApiError> {
    check_admin(&st, &headers)?;
    let ttl = ChronoDuration::seconds(st.cfg.leases.agent_ttl_seconds as i64);
    let now = Utc::now();

    let agents_raw = st.storage.list_agents().await.map_err(ApiError::Storage)?;
    let agents: Vec<AdminAgentView> = agents_raw
        .iter()
        .map(|a| {
            let status = match a.last_seen_at {
                Some(ls) if now - ls < ttl => "healthy",
                _ => "stale",
            };
            AdminAgentView {
                agent_id: a.agent_id.clone(),
                display_name: a.display_name.clone(),
                tailscale_ipv4: a.tailscale_ipv4.clone(),
                docker_engine_id: a.docker_engine_id.clone(),
                created_at: a.created_at,
                last_seen_at: a.last_seen_at,
                status: status.into(),
            }
        })
        .collect();

    let rows = st
        .storage
        .list_routes_admin(ttl)
        .await
        .map_err(ApiError::Storage)?;
    let ip_by_agent: std::collections::HashMap<_, _> = agents_raw
        .into_iter()
        .map(|a| (a.agent_id, a.tailscale_ipv4))
        .collect();

    let probe_timeout = std::time::Duration::from_millis(1500);
    let mut routes = Vec::with_capacity(rows.len());
    for (hostname, agent_id, backend, cid, cname, lease) in rows {
        let ts_ip = ip_by_agent.get(&agent_id).cloned().unwrap_or_default();
        let registration = if lease > now {
            "healthy".to_string()
        } else {
            "stale".to_string()
        };

        let (http_probe, probe_ms, probe_detail) = if ts_ip.is_empty() {
            ("skipped".into(), None, Some("no agent ip".into()))
        } else {
            let p = crate::probe::probe_via_agent(&ts_ip, &hostname, probe_timeout).await;
            (p.status, p.ms, p.detail)
        };

        routes.push(AdminRouteHealthView {
            hostname,
            agent_id,
            tailscale_ipv4: ts_ip,
            backend,
            container_id: cid,
            container_name: cname,
            lease_expires_at: lease,
            registration,
            http_probe,
            probe_ms,
            probe_detail,
        });
    }

    Ok(Json(AdminDashboard { routes, agents }))
}

async fn admin_delete_route(
    State(st): State<SharedState>,
    Path(hostname): Path<String>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    check_admin(&st, &headers)?;
    st.storage
        .delete_route(&hostname)
        .await
        .map_err(ApiError::Storage)?;
    refresh_dns(&st).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn admin_create_enrollment_token(
    State(st): State<SharedState>,
    headers: HeaderMap,
) -> Result<Json<CreateEnrollmentTokenResponse>, ApiError> {
    check_admin(&st, &headers)?;
    let rec = st
        .storage
        .create_enrollment_token()
        .await
        .map_err(ApiError::Storage)?;
    Ok(Json(CreateEnrollmentTokenResponse {
        token: rec.token_plain,
        expires_at: None,
    }))
}

async fn admin_revoke_agent(
    State(st): State<SharedState>,
    Path(agent_id): Path<String>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    check_admin(&st, &headers)?;
    st.storage
        .revoke_agent(&agent_id)
        .await
        .map_err(ApiError::Storage)?;
    refresh_dns(&st).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn refresh_dns(st: &AppState) -> Result<(), ApiError> {
    let ttl = ChronoDuration::seconds(st.cfg.leases.agent_ttl_seconds as i64);
    let routes = st
        .storage
        .active_dns_routes(ttl)
        .await
        .map_err(ApiError::Storage)?;
    let mut map = std::collections::HashMap::new();
    for r in routes {
        if let Ok(ip) = r.tailscale_ipv4.parse() {
            map.insert(r.hostname, ip);
        }
    }
    st.registry.replace(map);
    Ok(())
}

#[derive(Debug)]
enum ApiError {
    Unauthorized,
    Forbidden(String),
    Storage(tailsvc_storage::StorageError),
}

impl From<tailsvc_storage::StorageError> for ApiError {
    fn from(e: tailsvc_storage::StorageError) -> Self {
        ApiError::Storage(e)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        match self {
            ApiError::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized").into_response(),
            ApiError::Forbidden(msg) => (StatusCode::FORBIDDEN, msg).into_response(),
            ApiError::Storage(e) => {
                tracing::error!(error = %e, "storage error");
                (StatusCode::INTERNAL_SERVER_ERROR, "storage error").into_response()
            }
        }
    }
}
