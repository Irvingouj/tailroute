mod common;

use common::E2eHarness;
use tailsvc_common::api::RouteEntry;

#[tokio::test]
async fn enroll_register_dns_and_conflict() {
    let h = E2eHarness::new().await.expect("harness");
    let ts_ip = "100.64.0.10";

    let enroll1 = h.create_enrollment_token().await.expect("token1");
    let a1 = h.enroll(&enroll1, ts_ip).await.expect("enroll1");

    h.heartbeat(&a1.agent_id, &a1.agent_token, ts_ip)
        .await
        .expect("hb");

    let put = h
        .put_routes(
            &a1.agent_id,
            &a1.agent_token,
            vec![RouteEntry {
                hostname: "whoami.internal".into(),
                backend: "http://127.0.0.1:9".into(),
                container_id: Some("c1".into()),
                container_name: Some("whoami".into()),
            }],
        )
        .await
        .expect("routes");
    assert_eq!(put.accepted, vec!["whoami.internal"]);
    assert!(put.conflicts.is_empty());

    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    let reg = h.registry_snapshot();
    assert_eq!(
        reg.get("whoami.internal").map(|ip| ip.to_string()),
        Some(ts_ip.to_string())
    );
    let ips = h.dns_lookup_a("whoami.internal").await.expect("dns");
    assert_eq!(
        ips,
        vec!["100.64.0.10".parse::<std::net::Ipv4Addr>().unwrap()]
    );

    let enroll2 = h.create_enrollment_token().await.expect("token2");
    let a2 = h.enroll(&enroll2, "100.64.0.11").await.expect("enroll2");
    h.heartbeat(&a2.agent_id, &a2.agent_token, "100.64.0.11")
        .await
        .expect("hb2");

    let conflict = h
        .put_routes(
            &a2.agent_id,
            &a2.agent_token,
            vec![RouteEntry {
                hostname: "whoami.internal".into(),
                backend: "http://127.0.0.1:9".into(),
                container_id: None,
                container_name: None,
            }],
        )
        .await
        .expect("conflict put");
    assert!(conflict.accepted.is_empty());
    assert_eq!(conflict.conflicts.len(), 1);
    assert_eq!(conflict.conflicts[0].owner_agent_id, a1.agent_id);
}

#[tokio::test]
async fn enrollment_token_is_one_time() {
    let h = E2eHarness::new().await.expect("harness");
    let token = h.create_enrollment_token().await.expect("token");
    let _ = h.enroll(&token, "100.64.0.1").await.expect("first");
    let second = h.enroll(&token, "100.64.0.2").await;
    assert!(second.is_err());
}

#[tokio::test]
async fn lease_expiry_removes_dns_after_ttl() {
    // Use a short-TTL controller for this test only.
    // Bootstrap with default 90s TTL — exercise purge via storage.purge_expired_agents(0).
    let h = E2eHarness::new().await.unwrap();
    let ts = "100.64.0.30";
    let token = h.create_enrollment_token().await.unwrap();
    let ag = h.enroll(&token, ts).await.unwrap();
    h.heartbeat(&ag.agent_id, &ag.agent_token, ts)
        .await
        .unwrap();
    h.put_routes(
        &ag.agent_id,
        &ag.agent_token,
        vec![RouteEntry {
            hostname: "lease.internal".into(),
            backend: "http://127.0.0.1:1".into(),
            container_id: None,
            container_name: None,
        }],
    )
    .await
    .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(80)).await;
    assert!(!h.dns_lookup_a("lease.internal").await.unwrap().is_empty());

    // Force purge as if agent was long-expired.
    let ttl = chrono::Duration::seconds(0);
    h.controller
        .state
        .storage
        .purge_expired_agents(ttl)
        .await
        .unwrap();
    tailsvc_controller::refresh_dns_registry(&h.controller.state)
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let ips = h.dns_lookup_a("lease.internal").await.unwrap();
    assert!(ips.is_empty(), "expired lease must drop DNS");
}

#[tokio::test]
async fn route_removed_when_agent_replaces_with_empty_set() {
    let h = E2eHarness::new().await.expect("harness");
    let ts = "100.64.0.20";
    let token = h.create_enrollment_token().await.unwrap();
    let ag = h.enroll(&token, ts).await.unwrap();
    h.heartbeat(&ag.agent_id, &ag.agent_token, ts)
        .await
        .unwrap();
    h.put_routes(
        &ag.agent_id,
        &ag.agent_token,
        vec![RouteEntry {
            hostname: "temp.internal".into(),
            backend: "http://127.0.0.1:1".into(),
            container_id: None,
            container_name: None,
        }],
    )
    .await
    .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(80)).await;
    assert!(!h.dns_lookup_a("temp.internal").await.unwrap().is_empty());

    h.put_routes(&ag.agent_id, &ag.agent_token, vec![])
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(80)).await;
    let ips = h.dns_lookup_a("temp.internal").await.unwrap();
    assert!(ips.is_empty());
}
