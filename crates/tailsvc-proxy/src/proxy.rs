use crate::routes::{BackendRoute, RouteStore, SharedRouteStore};
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Empty, Full};
use hyper::body::{Bytes, Incoming};
use hyper::header::{HeaderMap, HeaderName, HeaderValue, CONNECTION, HOST, UPGRADE};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode, Version};
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::client::legacy::Client;
use hyper_util::rt::{TokioExecutor, TokioIo};
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tracing::{debug, info, warn};

type ProxyBody = BoxBody<Bytes, hyper::Error>;

#[derive(Clone)]
pub struct ProxyConfig {
    pub listen: SocketAddr,
    pub connect_timeout: Duration,
    pub response_timeout: Duration,
    pub routes: Arc<SharedRouteStore>,
    pub shutting_down: Arc<AtomicBool>,
}

pub struct ProxyServer {
    cfg: ProxyConfig,
}

impl ProxyServer {
    pub fn new(cfg: ProxyConfig) -> Self {
        Self { cfg }
    }

    pub async fn run(self) -> std::io::Result<()> {
        let listener = TcpListener::bind(self.cfg.listen).await?;
        info!(addr = %self.cfg.listen, "proxy listening");

        let mut connector = HttpConnector::new();
        connector.set_nodelay(true);
        connector.set_connect_timeout(Some(self.cfg.connect_timeout));
        // Body type Incoming: stream client request bodies to backend without buffering.
        let client: Client<HttpConnector, Incoming> =
            Client::builder(TokioExecutor::new()).build(connector);

        // Keep accepting while shutting down so in-flight and new requests get 503
        // (SPEC §5.3). Callers stop the process after a drain period.
        loop {
            let (stream, peer) = listener.accept().await?;
            let cfg = self.cfg.clone();
            let client = client.clone();
            tokio::spawn(async move {
                let io = TokioIo::new(stream);
                let svc = service_fn(move |req| {
                    let cfg = cfg.clone();
                    let client = client.clone();
                    async move { Ok::<_, Infallible>(handle(cfg, client, peer, req).await) }
                });
                if let Err(e) = http1::Builder::new()
                    .serve_connection(io, svc)
                    .with_upgrades()
                    .await
                {
                    debug!(error = %e, peer = %peer, "proxy connection closed");
                }
            });
        }
    }
}

async fn handle(
    cfg: ProxyConfig,
    client: Client<HttpConnector, Incoming>,
    peer: SocketAddr,
    req: Request<Incoming>,
) -> Response<ProxyBody> {
    if cfg.shutting_down.load(Ordering::Relaxed) {
        return text_response(StatusCode::SERVICE_UNAVAILABLE, "agent shutting down");
    }

    let host_hdr = req
        .headers()
        .get(HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let Some(route) = cfg.routes.resolve(host_hdr) else {
        return text_response(StatusCode::NOT_FOUND, "unknown tailsvc host");
    };

    if is_websocket_upgrade(&req) {
        return proxy_websocket(cfg, peer, req, route).await;
    }

    proxy_http(cfg, client, peer, req, route).await
}

fn is_websocket_upgrade(req: &Request<Incoming>) -> bool {
    let upgrade = req
        .headers()
        .get(UPGRADE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.eq_ignore_ascii_case("websocket"))
        .unwrap_or(false);
    let connection = req
        .headers()
        .get(CONNECTION)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_ascii_lowercase().contains("upgrade"))
        .unwrap_or(false);
    upgrade && connection
}

async fn proxy_http(
    cfg: ProxyConfig,
    client: Client<HttpConnector, Incoming>,
    peer: SocketAddr,
    mut req: Request<Incoming>,
    route: BackendRoute,
) -> Response<ProxyBody> {
    let authority = route.backend.authority();
    let path_q = req
        .uri()
        .path_and_query()
        .map(|p| p.as_str())
        .unwrap_or("/");
    let target_uri = match format!("http://{authority}{path_q}").parse::<hyper::Uri>() {
        Ok(u) => u,
        Err(_) => return text_response(StatusCode::BAD_REQUEST, "bad request"),
    };

    let original_host = req.headers().get(HOST).cloned();
    inject_forwarded_headers(req.headers_mut(), peer, original_host.as_ref());
    *req.uri_mut() = target_uri;

    let result = tokio::time::timeout(cfg.response_timeout, client.request(req)).await;

    match result {
        Ok(Ok(resp)) => {
            let (parts, body) = resp.into_parts();
            let mut builder = Response::builder()
                .status(parts.status)
                .version(parts.version);
            builder = builder.header(
                hyper::header::CACHE_CONTROL,
                "no-store, no-cache, must-revalidate",
            );
            for (name, value) in parts.headers.iter() {
                if name == hyper::header::TRANSFER_ENCODING {
                    continue;
                }
                builder = builder.header(name, value);
            }
            // Stream response body — do not buffer.
            let streamed: ProxyBody = body.map_err(|e| e).boxed();
            match builder.body(streamed) {
                Ok(r) => r,
                Err(_) => text_response(StatusCode::BAD_GATEWAY, "backend unavailable"),
            }
        }
        Ok(Err(e)) => {
            warn!(error = %e, backend = %authority, "backend error");
            let msg = e.to_string().to_ascii_lowercase();
            if msg.contains("timeout") || msg.contains("timed out") {
                return text_response(StatusCode::GATEWAY_TIMEOUT, "backend timeout");
            }
            text_response(StatusCode::BAD_GATEWAY, "backend unavailable")
        }
        Err(_) => text_response(StatusCode::GATEWAY_TIMEOUT, "backend timeout"),
    }
}

async fn proxy_websocket(
    cfg: ProxyConfig,
    peer: SocketAddr,
    mut req: Request<Incoming>,
    route: BackendRoute,
) -> Response<ProxyBody> {
    let authority = route.backend.authority();
    let path_q = req
        .uri()
        .path_and_query()
        .map(|p| p.as_str())
        .unwrap_or("/")
        .to_string();

    // Capture client upgrade before we rebuild/forward.
    let client_upgrade = hyper::upgrade::on(&mut req);

    let backend_addr = match resolve_authority(&authority).await {
        Ok(a) => a,
        Err(e) => {
            warn!(error = %e, "ws resolve backend");
            return text_response(StatusCode::BAD_GATEWAY, "backend unavailable");
        }
    };

    let connect = tokio::time::timeout(
        cfg.connect_timeout,
        tokio::net::TcpStream::connect(backend_addr),
    )
    .await;
    let backend_stream = match connect {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => {
            warn!(error = %e, "ws backend connect");
            return text_response(StatusCode::BAD_GATEWAY, "backend unavailable");
        }
        Err(_) => return text_response(StatusCode::GATEWAY_TIMEOUT, "backend timeout"),
    };

    let mut headers = req.headers().clone();
    let original_host = headers.get(HOST).cloned();
    inject_forwarded_headers(&mut headers, peer, original_host.as_ref());

    let mut req_builder = Request::builder()
        .method(Method::GET)
        .uri(path_q)
        .version(Version::HTTP_11);
    for (k, v) in headers.iter() {
        req_builder = req_builder.header(k, v);
    }
    let backend_req = match req_builder.body(Empty::<Bytes>::new()) {
        Ok(r) => r,
        Err(_) => return text_response(StatusCode::BAD_REQUEST, "bad request"),
    };

    let io = TokioIo::new(backend_stream);
    let (mut sender, conn) = match hyper::client::conn::http1::handshake(io).await {
        Ok(x) => x,
        Err(e) => {
            warn!(error = %e, "ws handshake");
            return text_response(StatusCode::BAD_GATEWAY, "backend unavailable");
        }
    };
    tokio::spawn(async move {
        let _ = conn.with_upgrades().await;
    });

    let backend_resp = match sender.send_request(backend_req).await {
        Ok(r) => r,
        Err(e) => {
            warn!(error = %e, "ws backend request");
            return text_response(StatusCode::BAD_GATEWAY, "backend unavailable");
        }
    };

    if backend_resp.status() != StatusCode::SWITCHING_PROTOCOLS {
        warn!(status = %backend_resp.status(), "ws backend refused upgrade");
        return text_response(StatusCode::BAD_GATEWAY, "backend unavailable");
    }

    // Copy 101 + upgrade headers back to client.
    let mut out = Response::builder().status(StatusCode::SWITCHING_PROTOCOLS);
    for (name, value) in backend_resp.headers().iter() {
        out = out.header(name, value);
    }
    let response = out
        .body(empty_body())
        .unwrap_or_else(|_| text_response(StatusCode::BAD_GATEWAY, "backend unavailable"));

    let backend_upgrade = hyper::upgrade::on(backend_resp);
    tokio::spawn(async move {
        let client_io = match client_upgrade.await {
            Ok(u) => u,
            Err(e) => {
                warn!(error = %e, "client upgrade failed");
                return;
            }
        };
        let backend_io = match backend_upgrade.await {
            Ok(u) => u,
            Err(e) => {
                warn!(error = %e, "backend upgrade failed");
                return;
            }
        };
        let mut client = TokioIo::new(client_io);
        let mut backend = TokioIo::new(backend_io);
        match tokio::io::copy_bidirectional(&mut client, &mut backend).await {
            Ok((a, b)) => debug!(up = a, down = b, "ws tunnel closed"),
            Err(e) => debug!(error = %e, "ws tunnel error"),
        }
        let _ = client.shutdown().await;
        let _ = backend.shutdown().await;
    });

    response
}

async fn resolve_authority(authority: &str) -> std::io::Result<SocketAddr> {
    if let Ok(a) = authority.parse::<SocketAddr>() {
        return Ok(a);
    }
    let mut it = tokio::net::lookup_host(authority).await?;
    it.next()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "no address"))
}

fn inject_forwarded_headers(
    headers: &mut HeaderMap,
    peer: SocketAddr,
    original_host: Option<&HeaderValue>,
) {
    if let Some(h) = original_host {
        headers.insert(HOST, h.clone());
    }

    headers.insert(
        HeaderName::from_static("x-forwarded-proto"),
        HeaderValue::from_static("http"),
    );
    if let Some(h) = original_host {
        if let Ok(v) = HeaderValue::from_bytes(h.as_bytes()) {
            headers.insert(HeaderName::from_static("x-forwarded-host"), v);
        }
    }

    let peer_ip = peer.ip().to_string();
    let xff = match headers.get("x-forwarded-for").and_then(|v| v.to_str().ok()) {
        Some(existing) if !existing.is_empty() => format!("{existing}, {peer_ip}"),
        _ => peer_ip,
    };
    if let Ok(v) = HeaderValue::from_str(&xff) {
        headers.insert(HeaderName::from_static("x-forwarded-for"), v);
    }
}

fn empty_body() -> ProxyBody {
    Empty::<Bytes>::new()
        .map_err(|never| match never {})
        .boxed()
}

fn text_response(status: StatusCode, body: &str) -> Response<ProxyBody> {
    let b: ProxyBody = Full::new(Bytes::from(body.to_string()))
        .map_err(|never| match never {})
        .boxed();
    Response::builder()
        .status(status)
        .header(hyper::header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .header(hyper::header::CACHE_CONTROL, "no-store")
        .body(b)
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_404_body() {
        let r = text_response(StatusCode::NOT_FOUND, "unknown tailsvc host");
        assert_eq!(r.status(), StatusCode::NOT_FOUND);
    }
}
