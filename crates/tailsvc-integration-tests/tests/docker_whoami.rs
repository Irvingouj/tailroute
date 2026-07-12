mod common;

// Run with: cargo test -p tailsvc-integration-tests docker_whoami -- --ignored

use common::E2eHarness;
use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;
use tailsvc_common::api::RouteEntry;
use tailsvc_docker::{backend_url, resolve_backend, DockerRuntime};

#[tokio::test]
#[ignore = "requires Docker daemon"]
async fn docker_labeled_whoami_reaches_backend_via_proxy() {
    if !docker_available() {
        eprintln!("skip: docker not available");
        return;
    }

    let container_name = format!("tailsvc-e2e-{}", uuid::Uuid::new_v4());
    let image = "traefik/whoami";
    pull_image(image);

    // Publish on localhost so the host-side proxy can reach the backend (bridge IPs are not routable on Docker Desktop macOS).
    let host_port = "19111";
    let backend_label = format!("http://127.0.0.1:{host_port}");
    let publish = format!("127.0.0.1:{host_port}:80");
    let run_out = std::process::Command::new("docker")
        .args([
            "run",
            "-d",
            "--name",
            &container_name,
            "-p",
            &publish,
            "--label",
            "tailsvc.enable=true",
            "--label",
            "tailsvc.hosts=whoami-e2e.internal",
            "--label",
            &format!("tailsvc.backend={backend_label}"),
            image,
        ])
        .output()
        .expect("docker run");
    assert!(
        run_out.status.success(),
        "docker run: {}",
        String::from_utf8_lossy(&run_out.stderr)
    );

    struct Cleanup(String);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::process::Command::new("docker")
                .args(["rm", "-f", &self.0])
                .status();
        }
    }
    let _cleanup = Cleanup(container_name.clone());

    tokio::time::sleep(Duration::from_secs(3)).await;

    let runtime = DockerRuntime::connect("/var/run/docker.sock").expect("docker connect");
    let services = runtime.list_services().await.expect("list");
    let whoami = services
        .iter()
        .find(|s| {
            s.labels
                .hosts
                .iter()
                .any(|h| h.as_str() == "whoami-e2e.internal")
        })
        .expect("labeled whoami container");
    let backend = resolve_backend(whoami).expect("resolve backend");

    let fake_ts = Ipv4Addr::new(100, 64, 0, 99);
    let proxy_listen = SocketAddr::from(([127, 0, 0, 1], 18081));
    let _proxy = common::spawn_proxy_on(proxy_listen, backend.clone(), "whoami-e2e.internal")
        .await
        .unwrap();

    let h = E2eHarness::new().await.unwrap();
    let enroll = h.create_enrollment_token().await.unwrap();
    let ag = h.enroll(&enroll, &fake_ts.to_string()).await.unwrap();
    h.heartbeat(&ag.agent_id, &ag.agent_token, &fake_ts.to_string())
        .await
        .unwrap();
    h.put_routes(
        &ag.agent_id,
        &ag.agent_token,
        vec![RouteEntry {
            hostname: "whoami-e2e.internal".into(),
            backend: backend_url(&backend),
            container_id: Some(whoami.container_id.clone()),
            container_name: Some(whoami.container_name.clone()),
        }],
    )
    .await
    .unwrap();

    let ips = h.dns_lookup_a("whoami-e2e.internal").await.unwrap();
    assert_eq!(ips[0], fake_ts);

    let resp = reqwest::Client::new()
        .get(format!("http://{proxy_listen}/"))
        .header("Host", "whoami-e2e.internal")
        .send()
        .await
        .unwrap();
    let status = resp.status();
    let text = resp.text().await.unwrap();
    assert!(status.is_success(), "proxy status {status} body {text}");
    assert!(
        text.contains("Hostname") || text.contains("whoami"),
        "unexpected body: {text}"
    );
}

fn docker_available() -> bool {
    let docker = std::env::var("PATH")
        .ok()
        .and_then(|p| {
            for dir in p.split(':') {
                let c = std::path::Path::new(dir).join("docker");
                if c.is_file() {
                    return Some(c);
                }
            }
            None
        })
        .unwrap_or_else(|| std::path::PathBuf::from("docker"));
    std::process::Command::new(docker)
        .args(["ps"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn pull_image(image: &str) {
    let _ = std::process::Command::new("docker")
        .args(["pull", image])
        .status();
}
