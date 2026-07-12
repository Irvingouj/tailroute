mod common;

use common::{spawn_fake_backend, spawn_proxy_on};
use http_body_util::{BodyExt, Full};
use hyper::body::Bytes;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use std::net::SocketAddr;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;
use tailsvc_common::backend::Backend;
use tailsvc_proxy::{BackendRoute, ProxyConfig, ProxyServer, SharedRouteStore};
use tokio::net::TcpListener;

async fn free_listen() -> SocketAddr {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let a = l.local_addr().unwrap();
    drop(l);
    a
}

async fn spawn_echo_headers_backend() -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            tokio::spawn(async move {
                let svc = service_fn(|req: Request<hyper::body::Incoming>| async move {
                    let host = req
                        .headers()
                        .get("host")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("")
                        .to_string();
                    let xff = req
                        .headers()
                        .get("x-forwarded-for")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("")
                        .to_string();
                    let xfh = req
                        .headers()
                        .get("x-forwarded-host")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("")
                        .to_string();
                    let xfp = req
                        .headers()
                        .get("x-forwarded-proto")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("")
                        .to_string();
                    let body = format!("host={host};xff={xff};xfh={xfh};xfp={xfp}");
                    Ok::<_, hyper::Error>(
                        Response::builder()
                            .status(StatusCode::OK)
                            .body(Full::new(Bytes::from(body)))
                            .unwrap(),
                    )
                });
                let _ = http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), svc)
                    .await;
            });
        }
    });
    (addr, handle)
}

async fn spawn_streaming_backend() -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            tokio::spawn(async move {
                let svc = service_fn(|req: Request<hyper::body::Incoming>| async move {
                    // Echo request body size then stream a large response.
                    let collected = req.into_body().collect().await.unwrap().to_bytes();
                    let mut out = format!("in={};", collected.len()).into_bytes();
                    out.extend(std::iter::repeat_n(b'x', 64 * 1024));
                    Ok::<_, hyper::Error>(
                        Response::builder()
                            .status(StatusCode::OK)
                            .header("content-type", "text/plain")
                            .body(Full::new(Bytes::from(out)))
                            .unwrap(),
                    )
                });
                let _ = http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), svc)
                    .await;
            });
        }
    });
    (addr, handle)
}

async fn start_proxy(
    backend: Backend,
    hostname: &str,
    shutting_down: Arc<AtomicBool>,
) -> (SocketAddr, Arc<SharedRouteStore>) {
    let listen = free_listen().await;
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
    let proxy = ProxyServer::new(ProxyConfig {
        listen,
        connect_timeout: Duration::from_secs(2),
        response_timeout: Duration::from_secs(10),
        routes: routes.clone(),
        shutting_down,
    });
    tokio::spawn(async move {
        let _ = proxy.run().await;
    });
    tokio::time::sleep(Duration::from_millis(40)).await;
    (listen, routes)
}

#[tokio::test]
async fn proxy_routes_by_host_through_registered_backend() {
    let (backend_addr, _backend_task) = spawn_fake_backend().await.expect("backend");
    let backend = Backend {
        host: backend_addr.ip().to_string(),
        port: backend_addr.port(),
    };
    let listen = free_listen().await;
    let _proxy = spawn_proxy_on(listen, backend, "whoami.internal")
        .await
        .expect("proxy");

    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://{listen}/"))
        .header("Host", "whoami.internal")
        .send()
        .await
        .expect("curl proxy");
    assert!(resp.status().is_success());
    assert_eq!(resp.text().await.unwrap(), "whoami-backend-ok");

    let unknown = client
        .get(format!("http://{listen}/"))
        .header("Host", "unknown.internal")
        .send()
        .await
        .unwrap();
    assert_eq!(unknown.status(), 404);
    assert_eq!(unknown.text().await.unwrap(), "unknown tailsvc host");
}

#[tokio::test]
async fn proxy_preserves_host_and_adds_forwarded_headers() {
    let (backend_addr, _) = spawn_echo_headers_backend().await;
    let backend = Backend {
        host: backend_addr.ip().to_string(),
        port: backend_addr.port(),
    };
    let shutting_down = Arc::new(AtomicBool::new(false));
    let (listen, _) = start_proxy(backend, "app.internal", shutting_down).await;

    let client = reqwest::Client::new();
    let body = client
        .get(format!("http://{listen}/path"))
        .header("Host", "app.internal")
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();

    assert!(
        body.contains("host=app.internal"),
        "Host must be preserved, got: {body}"
    );
    assert!(
        body.contains("xfh=app.internal"),
        "X-Forwarded-Host: {body}"
    );
    assert!(body.contains("xfp=http"), "X-Forwarded-Proto: {body}");
    assert!(body.contains("xff="), "X-Forwarded-For: {body}");
}

#[tokio::test]
async fn proxy_streams_large_request_and_response() {
    let (backend_addr, _) = spawn_streaming_backend().await;
    let backend = Backend {
        host: backend_addr.ip().to_string(),
        port: backend_addr.port(),
    };
    let shutting_down = Arc::new(AtomicBool::new(false));
    let (listen, _) = start_proxy(backend, "stream.internal", shutting_down).await;

    let payload = vec![b'y'; 32 * 1024];
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{listen}/upload"))
        .header("Host", "stream.internal")
        .body(payload)
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());
    let body = resp.bytes().await.unwrap();
    assert!(body.starts_with(b"in=32768;"));
    assert!(body.len() > 64 * 1024);
}

#[tokio::test]
async fn proxy_returns_503_when_shutting_down() {
    let (backend_addr, _) = spawn_fake_backend().await.unwrap();
    let backend = Backend {
        host: backend_addr.ip().to_string(),
        port: backend_addr.port(),
    };
    let shutting_down = Arc::new(AtomicBool::new(true));
    let (listen, _) = start_proxy(backend, "down.internal", shutting_down).await;

    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://{listen}/"))
        .header("Host", "down.internal")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 503);
    assert_eq!(resp.text().await.unwrap(), "agent shutting down");
}

#[tokio::test]
async fn proxy_502_when_backend_down() {
    let backend = Backend {
        host: "127.0.0.1".into(),
        port: 1, // nothing listening
    };
    let shutting_down = Arc::new(AtomicBool::new(false));
    let (listen, _) = start_proxy(backend, "dead.internal", shutting_down).await;

    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://{listen}/"))
        .header("Host", "dead.internal")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 502);
    assert_eq!(resp.text().await.unwrap(), "backend unavailable");
}

/// Minimal WebSocket echo backend: 101 + echo first frame-ish bytes.
async fn spawn_ws_echo_backend() -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            tokio::spawn(async move {
                let svc = service_fn(|mut req: Request<hyper::body::Incoming>| async move {
                    let upgrade = req
                        .headers()
                        .get("upgrade")
                        .and_then(|v| v.to_str().ok())
                        .map(|s| s.eq_ignore_ascii_case("websocket"))
                        .unwrap_or(false);
                    if !upgrade {
                        return Ok::<_, hyper::Error>(
                            Response::builder()
                                .status(StatusCode::BAD_REQUEST)
                                .body(Full::new(Bytes::from("not ws")))
                                .unwrap(),
                        );
                    }
                    let key = req
                        .headers()
                        .get("sec-websocket-key")
                        .cloned()
                        .unwrap_or_else(|| {
                            hyper::header::HeaderValue::from_static("dGhlIHNhbXBsZSBub25jZQ==")
                        });
                    let on_upgrade = hyper::upgrade::on(&mut req);
                    tokio::spawn(async move {
                        if let Ok(upgraded) = on_upgrade.await {
                            let mut io = TokioIo::new(upgraded);
                            use tokio::io::{AsyncReadExt, AsyncWriteExt};
                            let mut buf = [0u8; 256];
                            if let Ok(n) = io.read(&mut buf).await {
                                let _ = io.write_all(&buf[..n]).await;
                            }
                        }
                    });
                    // Accept key is not fully RFC6455 hashed here; clients that only
                    // check 101 still exercise the proxy upgrade path.
                    Ok(Response::builder()
                        .status(StatusCode::SWITCHING_PROTOCOLS)
                        .header("upgrade", "websocket")
                        .header("connection", "Upgrade")
                        .header("sec-websocket-accept", key)
                        .body(Full::new(Bytes::new()))
                        .unwrap())
                });
                let _ = http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), svc)
                    .with_upgrades()
                    .await;
            });
        }
    });
    (addr, handle)
}

#[tokio::test]
async fn proxy_websocket_upgrade_returns_101() {
    let (backend_addr, _) = spawn_ws_echo_backend().await;
    let backend = Backend {
        host: backend_addr.ip().to_string(),
        port: backend_addr.port(),
    };
    let shutting_down = Arc::new(AtomicBool::new(false));
    let (listen, _) = start_proxy(backend, "ws.internal", shutting_down).await;

    // Raw HTTP upgrade through the proxy.
    let mut stream = tokio::net::TcpStream::connect(listen).await.unwrap();
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let req = "GET /chat HTTP/1.1\r\n\
Host: ws.internal\r\n\
Upgrade: websocket\r\n\
Connection: Upgrade\r\n\
Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
Sec-WebSocket-Version: 13\r\n\r\n";
    stream.write_all(req.as_bytes()).await.unwrap();
    let mut buf = vec![0u8; 1024];
    let n = tokio::time::timeout(Duration::from_secs(3), stream.read(&mut buf))
        .await
        .expect("timeout")
        .unwrap();
    let resp = String::from_utf8_lossy(&buf[..n]);
    assert!(
        resp.contains("101"),
        "expected switching protocols, got: {resp}"
    );
}

#[tokio::test]
async fn proxy_sse_like_chunked_response() {
    // Backend returns text/event-stream body; proxy must stream without buffering forever.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            tokio::spawn(async move {
                let svc = service_fn(|_req: Request<hyper::body::Incoming>| async move {
                    let body = "data: one\n\ndata: two\n\n";
                    Ok::<_, hyper::Error>(
                        Response::builder()
                            .status(StatusCode::OK)
                            .header("content-type", "text/event-stream")
                            .header("cache-control", "no-cache")
                            .body(Full::new(Bytes::from(body)))
                            .unwrap(),
                    )
                });
                let _ = http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), svc)
                    .await;
            });
        }
    });
    let backend = Backend {
        host: addr.ip().to_string(),
        port: addr.port(),
    };
    let shutting_down = Arc::new(AtomicBool::new(false));
    let (listen, _) = start_proxy(backend, "sse.internal", shutting_down).await;
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://{listen}/events"))
        .header("Host", "sse.internal")
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());
    let ct = resp
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(ct.contains("text/event-stream"));
    let body = resp.text().await.unwrap();
    assert!(body.contains("data: one"));
    assert!(body.contains("data: two"));
}
