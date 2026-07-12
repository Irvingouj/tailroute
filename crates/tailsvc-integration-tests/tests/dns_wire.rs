//! DNS wire-format contract tests.
//!
//! Regression for split-DNS / MagicDNS clients (esp. macOS getaddrinfo):
//! responses MUST echo the question section. Header-only NODATA (MSG SIZE ~12,
//! dig shows QUERY: 0) makes dig sometimes look OK for A but curl hang.

mod common;

use common::dns_udp::{assert_well_formed_response, query_a, query_message};
use common::E2eHarness;
use hickory_proto::op::ResponseCode;
use hickory_proto::rr::RecordType;
use tailsvc_common::api::RouteEntry;

#[tokio::test]
async fn dns_a_response_includes_question_section() {
    let h = E2eHarness::new().await.expect("harness");
    let ts = "100.64.0.77";
    let token = h.create_enrollment_token().await.unwrap();
    let ag = h.enroll(&token, ts).await.unwrap();
    h.heartbeat(&ag.agent_id, &ag.agent_token, ts)
        .await
        .unwrap();
    h.put_routes(
        &ag.agent_id,
        &ag.agent_token,
        vec![RouteEntry {
            hostname: "wire-a.internal".into(),
            backend: "http://127.0.0.1:9".into(),
            container_id: None,
            container_name: None,
        }],
    )
    .await
    .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let (resp, len) = query_message(h.controller.dns_listen, "wire-a.internal", RecordType::A)
        .await
        .expect("dns a");
    assert_well_formed_response(&resp, len, "wire-a.internal", RecordType::A);
    assert_eq!(resp.response_code(), ResponseCode::NoError);
    assert!(
        !resp.answers().is_empty(),
        "registered A must have answer rr"
    );
    let ips = query_a(h.controller.dns_listen, "wire-a.internal")
        .await
        .unwrap();
    assert_eq!(ips, vec![ts.parse::<std::net::Ipv4Addr>().unwrap()]);
}

#[tokio::test]
async fn dns_aaaa_nodata_includes_question_and_is_not_header_only() {
    let h = E2eHarness::new().await.expect("harness");
    let ts = "100.64.0.78";
    let token = h.create_enrollment_token().await.unwrap();
    let ag = h.enroll(&token, ts).await.unwrap();
    h.heartbeat(&ag.agent_id, &ag.agent_token, ts)
        .await
        .unwrap();
    h.put_routes(
        &ag.agent_id,
        &ag.agent_token,
        vec![RouteEntry {
            hostname: "wire-aaaa.internal".into(),
            backend: "http://127.0.0.1:9".into(),
            container_id: None,
            container_name: None,
        }],
    )
    .await
    .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let (resp, len) = query_message(
        h.controller.dns_listen,
        "wire-aaaa.internal",
        RecordType::AAAA,
    )
    .await
    .expect("dns aaaa");

    // This is the macOS-breaking case when broken: MSG SIZE 12, QUERY: 0, no question.
    assert_well_formed_response(&resp, len, "wire-aaaa.internal", RecordType::AAAA);
    assert_eq!(
        resp.response_code(),
        ResponseCode::NoError,
        "v1 registered AAAA should be NODATA (NOERROR empty), not SERVFAIL/NXDOMAIN"
    );
    assert!(
        resp.answers().is_empty(),
        "v1 must not synthesize AAAA answers"
    );
    // Practical lower bound: header(12) + question (name + type/class) >> 12
    assert!(
        len >= 30,
        "AAAA NODATA wire too small ({len}); likely missing question section"
    );
}

#[tokio::test]
async fn dns_upstream_forward_includes_question_section() {
    let h = E2eHarness::new().await.expect("harness");
    // Unregistered public-ish name — forwarded upstream (needs network).
    let result = query_message(h.controller.dns_listen, "example.com", RecordType::A).await;
    let Ok((resp, len)) = result else {
        eprintln!("skip: upstream DNS unavailable: {result:?}");
        return;
    };
    if resp.response_code() == ResponseCode::ServFail {
        eprintln!("skip: upstream SERVFAIL");
        return;
    }
    assert_well_formed_response(&resp, len, "example.com", RecordType::A);
}
