use crate::registry::DnsRegistry;
use anyhow::{Context, Result};
use hickory_proto::op::{Message, Query, ResponseCode};
use hickory_proto::rr::{RData, Record, RecordType};
use hickory_proto::serialize::binary::{BinDecodable, BinEncodable};
use hickory_proto::xfer::Protocol;
use hickory_resolver::config::{NameServerConfig, ResolverConfig, ResolverOpts};
use hickory_resolver::name_server::TokioConnectionProvider;
use hickory_resolver::TokioResolver;
use std::collections::HashMap;
#[cfg(test)]
use std::net::Ipv4Addr;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::net::{TcpListener, UdpSocket};
use tracing::{debug, warn};

#[derive(Clone)]
pub struct DnsServerConfig {
    pub listen: SocketAddr,
    pub upstreams: Vec<SocketAddr>,
    pub service_ttl: u32,
    pub upstream_timeout: Duration,
    /// Cap for positive upstream TTLs (SPEC default 300s).
    pub positive_cache_max: Duration,
    /// Cap for negative upstream caching (SPEC default 30s).
    pub negative_cache_max: Duration,
    /// Overall per-query budget (SPEC default 5s).
    pub query_timeout: Duration,
    pub registry: DnsRegistry,
}

impl DnsServerConfig {
    pub fn with_defaults(
        listen: SocketAddr,
        upstreams: Vec<SocketAddr>,
        service_ttl: u32,
        upstream_timeout: Duration,
        registry: DnsRegistry,
    ) -> Self {
        Self {
            listen,
            upstreams,
            service_ttl,
            upstream_timeout,
            positive_cache_max: Duration::from_secs(300),
            negative_cache_max: Duration::from_secs(30),
            query_timeout: Duration::from_secs(5),
            registry,
        }
    }
}

#[derive(Clone)]
struct CacheEntry {
    records: Vec<Record>,
    rcode: ResponseCode,
    expires: Instant,
}

#[derive(Default)]
struct ResponseCache {
    map: Mutex<HashMap<(String, RecordType), CacheEntry>>,
}

impl ResponseCache {
    fn get(&self, name: &str, qtype: RecordType) -> Option<CacheEntry> {
        let key = (name.to_ascii_lowercase(), qtype);
        let mut guard = self.map.lock().ok()?;
        let ent = guard.get(&key)?.clone();
        if Instant::now() >= ent.expires {
            guard.remove(&key);
            return None;
        }
        Some(ent)
    }

    fn put(
        &self,
        name: &str,
        qtype: RecordType,
        records: Vec<Record>,
        rcode: ResponseCode,
        ttl: Duration,
    ) {
        if ttl.is_zero() {
            return;
        }
        let key = (name.to_ascii_lowercase(), qtype);
        if let Ok(mut guard) = self.map.lock() {
            guard.insert(
                key,
                CacheEntry {
                    records,
                    rcode,
                    expires: Instant::now() + ttl,
                },
            );
        }
    }
}

pub struct DnsServer {
    cfg: DnsServerConfig,
    resolver: TokioResolver,
    cache: Arc<ResponseCache>,
    listen_active: Arc<std::sync::atomic::AtomicBool>,
}

impl DnsServer {
    pub async fn new(cfg: DnsServerConfig) -> Result<Self> {
        let mut resolver_config = ResolverConfig::new();
        for upstream in &cfg.upstreams {
            // Prefer UDP; Hickory will try TCP on truncation as needed.
            resolver_config.add_name_server(NameServerConfig::new(*upstream, Protocol::Udp));
            resolver_config.add_name_server(NameServerConfig::new(*upstream, Protocol::Tcp));
        }
        let mut opts = ResolverOpts::default();
        opts.timeout = cfg.upstream_timeout;
        opts.attempts = 2;
        opts.cache_size = 0; // we implement our own capped cache
        let resolver =
            TokioResolver::builder_with_config(resolver_config, TokioConnectionProvider::default())
                .with_options(opts)
                .build();
        Ok(Self {
            cfg,
            resolver,
            cache: Arc::new(ResponseCache::default()),
            listen_active: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        })
    }

    pub fn is_listening(&self) -> bool {
        self.listen_active
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    pub async fn run(self) -> Result<()> {
        let udp = UdpSocket::bind(self.cfg.listen)
            .await
            .with_context(|| format!("bind udp {}", self.cfg.listen))?;
        let tcp = TcpListener::bind(self.cfg.listen)
            .await
            .with_context(|| format!("bind tcp {}", self.cfg.listen))?;
        self.listen_active
            .store(true, std::sync::atomic::Ordering::Relaxed);

        let this = Arc::new(self);
        let u = Arc::clone(&this);
        let udp = Arc::new(udp);
        tokio::spawn(async move {
            if let Err(e) = u.serve_udp(udp).await {
                warn!(error = %e, "dns udp server ended");
            }
        });
        this.serve_tcp(tcp).await
    }

    async fn serve_udp(self: Arc<Self>, sock: Arc<UdpSocket>) -> Result<()> {
        let mut buf = vec![0u8; 4096];
        loop {
            let (len, peer) = sock.recv_from(&mut buf).await?;
            let req_bytes = buf[..len].to_vec();
            let s = Arc::clone(&self);
            let sock = Arc::clone(&sock);
            tokio::spawn(async move {
                if let Ok(resp) = s.handle_request(&req_bytes, Some(peer)).await {
                    let _ = sock.send_to(&resp, peer).await;
                }
            });
        }
    }

    async fn serve_tcp(self: Arc<Self>, listener: TcpListener) -> Result<()> {
        loop {
            let (mut stream, peer) = listener.accept().await?;
            let s = Arc::clone(&self);
            tokio::spawn(async move {
                let _ = s.handle_tcp_conn(&mut stream, peer).await;
            });
        }
    }

    async fn handle_tcp_conn(
        self: &Arc<Self>,
        stream: &mut tokio::net::TcpStream,
        peer: SocketAddr,
    ) -> Result<()> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut len_buf = [0u8; 2];
        loop {
            if stream.read_exact(&mut len_buf).await.is_err() {
                break;
            }
            let len = u16::from_be_bytes(len_buf) as usize;
            let mut buf = vec![0u8; len];
            stream.read_exact(&mut buf).await?;
            let resp = self.handle_request(&buf, Some(peer)).await?;
            let out_len = (resp.len() as u16).to_be_bytes();
            stream.write_all(&out_len).await?;
            stream.write_all(&resp).await?;
        }
        Ok(())
    }

    async fn handle_request(
        self: &Arc<Self>,
        wire: &[u8],
        peer: Option<SocketAddr>,
    ) -> Result<Vec<u8>> {
        let req = Message::from_bytes(wire).context("decode dns")?;
        let mut resp = Message::new();
        resp.set_id(req.id());
        resp.set_message_type(hickory_proto::op::MessageType::Response);
        resp.set_op_code(req.op_code());
        resp.set_authoritative(false);
        resp.set_recursion_available(true);
        if req.recursion_desired() {
            resp.set_recursion_desired(true);
        }

        // RFC 1035: echo questions. Missing question section → QUERY:0 / tiny
        // wire packets; dig may still print answers but macOS getaddrinfo/curl hang.
        for q in req.queries() {
            resp.add_query(q.clone());
        }

        // DNS loop detection: refuse queries from our own listen IP on DNS port
        // when the query would recurse (heuristic for self-forward).
        if let Some(peer) = peer {
            if peer.ip() == self.cfg.listen.ip() && peer.port() == self.cfg.listen.port() {
                resp.set_response_code(ResponseCode::Refused);
                return Ok(resp.to_bytes()?);
            }
            // Also refuse if peer is one of our upstreams that might bounce back — skip.
            // If registry miss would forward to upstream equal to self, SERVFAIL.
            for up in &self.cfg.upstreams {
                if up.ip() == self.cfg.listen.ip() && up.port() == self.cfg.listen.port() {
                    // Misconfiguration: upstream points at self.
                    if req
                        .queries()
                        .iter()
                        .any(|q| !self.cfg.registry.contains(&q.name().to_ascii()))
                    {
                        resp.set_response_code(ResponseCode::ServFail);
                        return Ok(resp.to_bytes()?);
                    }
                }
            }
        }

        let mut any_fail = false;
        let mut any_answer = false;
        for q in req.queries().to_vec() {
            match tokio::time::timeout(self.cfg.query_timeout, self.answer_query(&q)).await {
                Ok(Ok((records, rcode))) => {
                    if rcode != ResponseCode::NoError && records.is_empty() {
                        resp.set_response_code(rcode);
                    }
                    for r in records {
                        resp.add_answer(r);
                        any_answer = true;
                    }
                }
                Ok(Err(e)) => {
                    debug!(error = %e, "query failed");
                    any_fail = true;
                }
                Err(_) => {
                    debug!("query overall timeout");
                    any_fail = true;
                }
            }
        }

        if any_fail && !any_answer {
            // SPEC: if all upstreams fail → SERVFAIL, do not fabricate NXDOMAIN.
            resp.set_response_code(ResponseCode::ServFail);
        }

        Ok(resp.to_bytes()?)
    }

    async fn answer_query(&self, query: &Query) -> Result<(Vec<Record>, ResponseCode)> {
        let name = query.name().to_ascii();
        let qtype = query.query_type();

        if self.cfg.registry.contains(&name) {
            return match qtype {
                RecordType::A => {
                    let ip = self
                        .cfg
                        .registry
                        .lookup_a(&name)
                        .ok_or_else(|| anyhow::anyhow!("missing registry ip"))?;
                    let rdata = RData::A(hickory_proto::rr::rdata::A(ip));
                    Ok((
                        vec![Record::from_rdata(
                            query.name().clone(),
                            self.cfg.service_ttl,
                            rdata,
                        )],
                        ResponseCode::NoError,
                    ))
                }
                RecordType::AAAA => Ok((vec![], ResponseCode::NoError)), // NODATA
                _ => self.forward_query(query).await,
            };
        }

        self.forward_query(query).await
    }

    async fn forward_query(&self, query: &Query) -> Result<(Vec<Record>, ResponseCode)> {
        let name_str = query.name().to_ascii();
        let qtype = query.query_type();

        if let Some(hit) = self.cache.get(&name_str, qtype) {
            return Ok((hit.records, hit.rcode));
        }

        let name = query.name().clone();
        let lookup = match self.resolver.lookup(name.clone(), qtype).await {
            Ok(l) => l,
            Err(e) => {
                // Negative cache NXDOMAIN / NoRecords
                let rcode = if e.is_no_records_found() {
                    ResponseCode::NXDomain
                } else {
                    return Err(anyhow::anyhow!("upstream lookup: {e}"));
                };
                self.cache
                    .put(&name_str, qtype, vec![], rcode, self.cfg.negative_cache_max);
                return Ok((vec![], rcode));
            }
        };

        let records: Vec<Record> = lookup.record_iter().cloned().collect();
        let ttl = records
            .iter()
            .map(|r| Duration::from_secs(u64::from(r.ttl())))
            .min()
            .unwrap_or(self.cfg.positive_cache_max)
            .min(self.cfg.positive_cache_max);
        self.cache.put(
            &name_str,
            qtype,
            records.clone(),
            ResponseCode::NoError,
            ttl,
        );
        Ok((records, ResponseCode::NoError))
    }
}

#[cfg(test)]
pub fn parse_ipv4(s: &str) -> Option<Ipv4Addr> {
    s.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ok() {
        assert_eq!(parse_ipv4("100.64.0.1"), Some(Ipv4Addr::new(100, 64, 0, 1)));
    }
}
