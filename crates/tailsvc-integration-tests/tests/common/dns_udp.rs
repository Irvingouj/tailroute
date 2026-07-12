#![allow(dead_code)]
use hickory_proto::op::{Message, MessageType, OpCode, Query};
use hickory_proto::rr::{Name, RecordType};
use hickory_proto::serialize::binary::{BinDecodable, BinEncodable};
use std::net::{Ipv4Addr, SocketAddr};
use tokio::net::UdpSocket;

async fn query_raw(
    server: SocketAddr,
    hostname: &str,
    qtype: RecordType,
) -> anyhow::Result<(Message, usize)> {
    let sock = UdpSocket::bind("127.0.0.1:0").await?;
    let mut msg = Message::new();
    msg.set_id(0x2a2a);
    msg.set_message_type(MessageType::Query);
    msg.set_op_code(OpCode::Query);
    msg.set_recursion_desired(true);
    let name = Name::from_ascii(format!("{hostname}."))?;
    msg.add_query(Query::query(name, qtype));

    let wire = msg.to_bytes()?;
    sock.send_to(&wire, server).await?;

    let mut buf = vec![0u8; 2048];
    let (len, _) =
        tokio::time::timeout(std::time::Duration::from_secs(3), sock.recv_from(&mut buf)).await??;

    let resp = Message::from_bytes(&buf[..len])?;
    Ok((resp, len))
}

pub async fn query_a(server: SocketAddr, hostname: &str) -> anyhow::Result<Vec<Ipv4Addr>> {
    let (resp, _) = query_raw(server, hostname, RecordType::A).await?;
    let mut out = Vec::new();
    for ans in resp.answers() {
        if let Some(a) = ans.data().as_a() {
            out.push(a.0);
        }
    }
    Ok(out)
}

/// Full DNS response for wire-format assertions (RFC 1035 question section, etc.).
pub async fn query_message(
    server: SocketAddr,
    hostname: &str,
    qtype: RecordType,
) -> anyhow::Result<(Message, usize)> {
    query_raw(server, hostname, qtype).await
}

/// Assert response is a well-formed answer for clients like macOS getaddrinfo:
/// - echoes the question (QUERY count >= 1)
/// - reasonable wire size (not header-only ~12 bytes)
pub fn assert_well_formed_response(
    resp: &Message,
    wire_len: usize,
    hostname: &str,
    qtype: RecordType,
) {
    assert!(
        !resp.queries().is_empty(),
        "DNS response must include question section (got QUERY:{}). \
         Malformed replies break macOS getaddrinfo/curl while dig may still show answers. \
         host={hostname} type={qtype:?} wire_len={wire_len}",
        resp.queries().len()
    );
    assert_eq!(resp.queries().len(), 1, "expected single question echoed");
    let q = &resp.queries()[0];
    assert_eq!(q.query_type(), qtype);
    let qname = q
        .name()
        .to_string()
        .trim_end_matches('.')
        .to_ascii_lowercase();
    let want = hostname.trim_end_matches('.').to_ascii_lowercase();
    assert_eq!(qname, want, "question name must match query");

    // Header alone is 12 bytes; a proper response with question is larger.
    assert!(
        wire_len > 12,
        "DNS response wire length {wire_len} looks like header-only (malformed NODATA/empty)"
    );
    assert_eq!(resp.message_type(), MessageType::Response);
}
