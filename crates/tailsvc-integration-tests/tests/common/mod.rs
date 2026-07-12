#![allow(dead_code)] // shared helpers; not every test binary uses every item
pub mod dns_udp;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;
use tailsvc_common::api::{
    EnrollRequest, EnrollResponse, PutRoutesRequest, PutRoutesResponse, RouteEntry,
};
use tailsvc_common::backend::Backend;
use tailsvc_controller::ControllerBootstrap;
use tailsvc_proxy::{BackendRoute, ProxyConfig, ProxyServer, SharedRouteStore};
use tokio::net::TcpListener;

pub struct E2eHarness {
    pub controller: ControllerBootstrap,
    pub admin_token: String,
    pub work_dir: tempfile::TempDir,
}

impl E2eHarness {
    pub async fn new() -> anyhow::Result<Self> {
        let work_dir = tempfile::tempdir()?;
        let db = work_dir.path().join("controller.db");
        let admin_token = "test-admin-token".to_string();
        let controller =
            ControllerBootstrap::start_test(db.to_str().unwrap(), &admin_token).await?;
        Ok(Self {
            controller,
            admin_token,
            work_dir,
        })
    }

    pub fn api(&self) -> String {
        self.controller.api_base()
    }

    pub async fn create_enrollment_token(&self) -> anyhow::Result<String> {
        let url = format!("{}/v1/admin/enrollment-tokens", self.api());
        let resp = reqwest::Client::new()
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.admin_token))
            .send()
            .await?;
        anyhow::ensure!(resp.status().is_success());
        let body: serde_json::Value = resp.json().await?;
        Ok(body["token"].as_str().unwrap().to_string())
    }

    pub async fn enroll(
        &self,
        enrollment_token: &str,
        tailscale_ip: &str,
    ) -> anyhow::Result<EnrollResponse> {
        let url = format!("{}/v1/agents/enroll", self.api());
        let resp = reqwest::Client::new()
            .post(&url)
            .header("Authorization", format!("Bearer {enrollment_token}"))
            .json(&EnrollRequest {
                display_name: "test-agent".into(),
                tailscale_ipv4: tailscale_ip.into(),
                docker_engine_id: "test-engine".into(),
            })
            .send()
            .await?;
        anyhow::ensure!(resp.status().is_success(), "enroll {}", resp.status());
        Ok(resp.json().await?)
    }

    pub async fn put_routes(
        &self,
        agent_id: &str,
        agent_token: &str,
        routes: Vec<RouteEntry>,
    ) -> anyhow::Result<PutRoutesResponse> {
        let url = format!("{}/v1/agents/{agent_id}/routes", self.api());
        let resp = reqwest::Client::new()
            .put(&url)
            .header("Authorization", format!("Bearer {agent_token}"))
            .json(&PutRoutesRequest { routes })
            .send()
            .await?;
        anyhow::ensure!(resp.status().is_success());
        Ok(resp.json().await?)
    }

    pub async fn heartbeat(
        &self,
        agent_id: &str,
        agent_token: &str,
        tailscale_ip: &str,
    ) -> anyhow::Result<()> {
        let url = format!("{}/v1/agents/{agent_id}/heartbeat", self.api());
        let resp = reqwest::Client::new()
            .post(&url)
            .header("Authorization", format!("Bearer {agent_token}"))
            .json(&tailsvc_common::api::HeartbeatRequest {
                tailscale_ipv4: tailscale_ip.into(),
                routes: vec![],
            })
            .send()
            .await?;
        anyhow::ensure!(resp.status().is_success());
        Ok(())
    }

    pub async fn dns_lookup_a(&self, hostname: &str) -> anyhow::Result<Vec<Ipv4Addr>> {
        dns_udp::query_a(self.controller.dns_listen, hostname).await
    }

    pub fn registry_snapshot(&self) -> std::collections::HashMap<String, Ipv4Addr> {
        self.controller.state.registry.snapshot()
    }
}

pub async fn spawn_fake_backend() -> anyhow::Result<(SocketAddr, JoinHandle)> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let handle = tokio::spawn(async move {
        use http_body_util::Full;
        use hyper::body::Bytes;
        use hyper::server::conn::http1;
        use hyper::service::service_fn;
        use hyper::{Request, Response, StatusCode};
        use hyper_util::rt::TokioIo;
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            tokio::spawn(async move {
                let svc = service_fn(|_req: Request<hyper::body::Incoming>| async {
                    Ok::<_, hyper::Error>(
                        Response::builder()
                            .status(StatusCode::OK)
                            .body(Full::new(Bytes::from("whoami-backend-ok")))
                            .unwrap(),
                    )
                });
                let _ = http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), svc)
                    .await;
            });
        }
    });
    Ok((addr, handle))
}

type JoinHandle = tokio::task::JoinHandle<()>;

pub async fn spawn_proxy_on(
    listen: SocketAddr,
    backend: Backend,
    hostname: &str,
) -> anyhow::Result<(Arc<SharedRouteStore>, JoinHandle)> {
    let routes = Arc::new(SharedRouteStore::new());
    let mut map = std::collections::HashMap::new();
    map.insert(
        hostname.to_string(),
        BackendRoute {
            backend,
            container_id: None,
            container_name: None,
        },
    );
    routes.replace(map);
    let shutting_down = Arc::new(AtomicBool::new(false));
    let proxy = ProxyServer::new(ProxyConfig {
        listen,
        connect_timeout: Duration::from_secs(2),
        response_timeout: Duration::from_secs(10),
        routes: routes.clone(),
        shutting_down,
    });
    let handle = tokio::spawn(async move {
        let _ = proxy.run().await;
    });
    tokio::time::sleep(Duration::from_millis(30)).await;
    Ok((routes, handle))
}
