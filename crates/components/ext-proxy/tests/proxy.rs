//! Integration acceptance tests for Layer 3 reverse proxy (`ext-proxy`):
//! - GA11: keep-alive connection pool reuse, Forwarded/X-Forwarded-* headers, strip_prefix
//! - GA12: active health probes & passive ejection failover + recovery
//! - GA13: SSE zero-buffer chunked passthrough & WebSocket 101 bidirectional proxy tunnel
//! - GA14: L4 TCP proxy forwarding & upstream header timeout -> 504 Gateway Timeout

use d_ext_proxy::{
    BackendConfig, HealthCheckConfig, HttpProxyRouteConfig,
    LoadBalanceStrategy, ProxyConfig, ProxyPlugin, TcpProxyRouteConfig,
    UpstreamConfig, UpstreamPoolService,
};
use frame::{ConfigValue, Context, ErrorKind, FrameOptions, Plugin};
use http_api::{
    HttpResponse, RouteMatch, RouteSpec, RouterService, StatusCode,
    stream_channel,
};
use l4_transport::{TokioSpawner, TransportConfig, TransportPlugin};
use proto_http::{BoundServerInfo, HttpConfig, HttpEnginePlugin, fn_handler};
use proto_ws::frame::{decode_frame, encode_frame};
use proto_ws::{WsConfig, WsEnginePlugin};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Notify;
use ws_api::{WsHandler, WsMessage, WsOpcode, WsRegistryService, WsSession};

async fn send_raw_http(addr: SocketAddr, req: &str) -> String {
    let mut stream = TcpStream::connect(addr).await.expect("connect");
    stream.write_all(req.as_bytes()).await.expect("write");
    let mut out = Vec::new();
    stream.read_to_end(&mut out).await.expect("read");
    String::from_utf8_lossy(&out).into_owned()
}

struct WsEchoHandler;

impl WsHandler for WsEchoHandler {
    fn on_connect(&self, session: WsSession) -> frame::BoxFuture<'static, ()> {
        Box::pin(async move {
            while let Some(Ok(msg)) = session.recv().await {
                match msg {
                    WsMessage::Text(s) => {
                        let _ = session
                            .send(WsMessage::Text(format!("proxied:{s}")))
                            .await;
                    }
                    WsMessage::Close(_) => break,
                    _ => {}
                }
            }
        })
    }
}

#[tokio::test]
async fn proxy_keepalive_headers_strip_prefix_sse_and_ws_tunnel() {
    // Reject unknown upstream ref in validate_config.
    let bad_cfg = ConfigValue::Map(BTreeMap::from([(
        "http_routes".to_string(),
        ConfigValue::List(vec![ConfigValue::Map(BTreeMap::from([
            ("id".to_string(), ConfigValue::Str("r1".into())),
            ("upstream".to_string(), ConfigValue::Str("missing".into())),
        ]))]),
    )]));
    assert_eq!(
        ProxyPlugin::default()
            .validate_config(&bad_cfg)
            .expect_err("unknown upstream must fail")
            .kind,
        ErrorKind::ValidationFailed
    );

    // 1. Start a persistent HTTP/1.1 + SSE + WS backend server using our own proto-http + proto-ws!
    let backend_ctx = Context::new(FrameOptions::new(Arc::new(TokioSpawner)));
    backend_ctx
        .plugin(
            TransportPlugin::new(TransportConfig {
                short_drain_timeout: Duration::from_millis(40),
                long_drain_timeout: Duration::from_millis(60),
                max_connections: None,
            }),
            ConfigValue::Null,
        )
        .unwrap();
    backend_ctx
        .plugin(
            HttpEnginePlugin::new(HttpConfig {
                bind_addr: Some("127.0.0.1:0".parse().unwrap()),
                ..HttpConfig::default()
            }),
            ConfigValue::Null,
        )
        .unwrap();
    backend_ctx
        .plugin(WsEnginePlugin::new(WsConfig::default()), ConfigValue::Null)
        .unwrap();
    backend_ctx.settle().await;

    let b_router = backend_ctx.get::<RouterService>().unwrap().as_ref().clone();
    let b_ws = backend_ctx
        .get::<WsRegistryService>()
        .unwrap()
        .as_ref()
        .clone();
    let b_transport = backend_ctx
        .get::<net_api::TransportService>()
        .unwrap()
        .as_ref()
        .clone();
    let backend_addr = backend_ctx.get::<BoundServerInfo>().unwrap().local_addr;

    let sse_gate = Arc::new(Notify::new());
    let sse_gate_r = Arc::clone(&sse_gate);

    let _h1 = b_router
        .mount_route(
            &backend_ctx,
            RouteSpec::new("echo", RouteMatch::Exact("/hello".into())),
            fn_handler(|req| async move {
                let xff = req.headers.get("x-forwarded-for").unwrap_or("");
                let xfh = req.headers.get("x-forwarded-host").unwrap_or("");
                let fwd = req.headers.get("forwarded").unwrap_or("");
                let extra = req.headers.get("x-proxy-id").unwrap_or("");
                HttpResponse::text(
                    StatusCode::OK,
                    format!(
                        "path={}|xff={xff}|xfh={xfh}|fwd={fwd}|extra={extra}",
                        req.raw_uri
                    ),
                )
            }),
        )
        .unwrap();

    let _h2 = b_router
        .mount_route(
            &backend_ctx,
            RouteSpec::new("sse", RouteMatch::Exact("/sse".into())),
            fn_handler(move |_| {
                let gate = Arc::clone(&sse_gate_r);
                async move {
                    let (tx, rx) = stream_channel();
                    tokio::spawn(async move {
                        let _ = tx.send(b"data: part1\n\n".to_vec());
                        gate.notified().await;
                        let _ = tx.send(b"data: part2\n\n".to_vec());
                        tx.close();
                    });
                    HttpResponse::stream(
                        StatusCode::OK,
                        "text/event-stream",
                        rx,
                        None,
                        true,
                    )
                }
            }),
        )
        .unwrap();

    let _h3 = b_ws
        .mount_ws(
            &backend_ctx,
            RouteSpec::new("ws", RouteMatch::Exact("/ws".into())),
            Arc::new(WsEchoHandler),
        )
        .unwrap();

    // Reserve a free L4 TCP proxy port forwarding to `backend_addr`.
    let l4_probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let tcp_proxy_addr = l4_probe.local_addr().unwrap();
    drop(l4_probe);

    // 2. Start edge proxy gateway using ext-proxy.
    let proxy_ctx = Context::new(FrameOptions::new(Arc::new(TokioSpawner)));
    proxy_ctx
        .plugin(
            TransportPlugin::new(TransportConfig {
                short_drain_timeout: Duration::from_millis(40),
                long_drain_timeout: Duration::from_millis(60),
                max_connections: None,
            }),
            ConfigValue::Null,
        )
        .unwrap();
    proxy_ctx
        .plugin(
            HttpEnginePlugin::new(HttpConfig {
                bind_addr: Some("127.0.0.1:0".parse().unwrap()),
                ..HttpConfig::default()
            }),
            ConfigValue::Null,
        )
        .unwrap();
    proxy_ctx
        .plugin(
            ProxyPlugin::new(ProxyConfig {
                upstreams: vec![UpstreamConfig {
                    max_idle_per_backend: 4,
                    ..UpstreamConfig::new(
                        "main",
                        vec![BackendConfig::plain(backend_addr)],
                    )
                }],
                http_routes: vec![HttpProxyRouteConfig {
                    id: "api".to_string(),
                    host: None,
                    match_prefix: "/api".to_string(),
                    strip_prefix: Some("/api".to_string()),
                    preserve_host: false,
                    upstream: "main".to_string(),
                    websocket: true,
                    retry_idempotent: true,
                    extra_headers: vec![(
                        "X-Proxy-Id".to_string(),
                        "gw-1".to_string(),
                    )],
                }],
                tcp_routes: vec![TcpProxyRouteConfig {
                    id: "tcp-edge".to_string(),
                    listen_addr: tcp_proxy_addr,
                    tls_listener_id: None,
                    max_connections: None,
                    upstream: "main".to_string(),
                }],
            }),
            ConfigValue::Null,
        )
        .unwrap();
    proxy_ctx.settle().await;
    let proxy_addr = proxy_ctx.get::<BoundServerInfo>().unwrap().local_addr;

    // GA11: strip_prefix + forwarding headers + keep-alive pool reuse (`total_accepted` stays 1 across 3 requests!)
    for i in 0..3 {
        let resp = send_raw_http(
            proxy_addr,
            &format!("GET /api/hello?n={i} HTTP/1.1\r\nHost: edge.local\r\nConnection: close\r\n\r\n"),
        )
        .await;
        assert!(resp.contains(&format!("path=/hello?n={i}|")), "{resp}");
        assert!(resp.contains("xff=127.0.0.1|"), "{resp}");
        assert!(resp.contains("xfh=edge.local|"), "{resp}");
        assert!(resp.contains("fwd=for=127.0.0.1;proto=http|"), "{resp}");
        assert!(resp.contains("extra=gw-1"), "{resp}");
    }
    assert_eq!(
        b_transport.stats().total_accepted,
        1,
        "keep-alive connection pool must reuse the single upstream connection"
    );

    // GA13a: Zero-buffer SSE passthrough through ext-proxy!
    let mut sse_stream = TcpStream::connect(proxy_addr).await.unwrap();
    sse_stream
        .write_all(b"GET /api/sse HTTP/1.1\r\nHost: edge.local\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut sse_buf = Vec::new();
    let mut tmp = [0u8; 512];
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    loop {
        let txt = String::from_utf8_lossy(&sse_buf);
        if txt.contains("data: part1\n\n") {
            assert!(!txt.contains("part2"), "part2 must wait for gate");
            break;
        }
        let rem =
            deadline.saturating_duration_since(tokio::time::Instant::now());
        assert!(!rem.is_zero(), "SSE part1 timed out: {txt}");
        let n = tokio::time::timeout(rem, sse_stream.read(&mut tmp))
            .await
            .unwrap()
            .unwrap();
        assert!(n > 0);
        sse_buf.extend_from_slice(&tmp[..n]);
    }
    sse_gate.notify_one();
    let mut tail = Vec::new();
    sse_stream.read_to_end(&mut tail).await.unwrap();
    sse_buf.extend_from_slice(&tail);
    assert!(
        String::from_utf8_lossy(&sse_buf).contains("data: part2\n\n"),
        "missing SSE part2"
    );

    // GA13b: End-to-end WebSocket 101 tunnel through ext-proxy -> proto-ws backend!
    let mut ws_conn = TcpStream::connect(proxy_addr).await.unwrap();
    ws_conn
        .write_all(
            b"GET /api/ws HTTP/1.1\r\nHost: edge.local\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n",
        )
        .await
        .unwrap();
    let mut ws_buf = Vec::new();
    loop {
        let n = ws_conn.read(&mut tmp).await.unwrap();
        assert!(n > 0);
        ws_buf.extend_from_slice(&tmp[..n]);
        if let Some(pos) = ws_buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&ws_buf[..pos]);
            assert!(head.starts_with("HTTP/1.1 101 "), "{head}");
            ws_buf.drain(..pos + 4);
            break;
        }
    }
    let mask = [0x21, 0x43, 0x65, 0x87];
    ws_conn
        .write_all(&encode_frame(true, WsOpcode::Text, b"hello-ws", Some(mask)))
        .await
        .unwrap();
    loop {
        if let Ok((f, used)) = decode_frame(&ws_buf, 65536) {
            ws_buf.drain(..used);
            if f.opcode == WsOpcode::Text {
                assert_eq!(f.payload, b"proxied:hello-ws");
                break;
            }
        }
        let n = ws_conn.read(&mut tmp).await.unwrap();
        assert!(n > 0);
        ws_buf.extend_from_slice(&tmp[..n]);
    }
    drop(ws_conn);

    // GA14a: L4 TCP reverse proxy listener forwards HTTP to upstream seamlessly!
    let tcp_resp = send_raw_http(
        tcp_proxy_addr,
        "GET /hello HTTP/1.1\r\nHost: raw-tcp\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(tcp_resp.contains("HTTP/1.1 200 OK"), "{tcp_resp}");
    assert!(tcp_resp.contains("path=/hello|"), "{tcp_resp}");

    proxy_ctx.dispose().await.unwrap();
    backend_ctx.dispose().await.unwrap();
}

#[tokio::test]
async fn proxy_active_health_check_and_timeout_504() {
    let b1_ok = Arc::new(AtomicBool::new(false));
    let b1_ok_srv = Arc::clone(&b1_ok);
    let b1_hits = Arc::new(AtomicUsize::new(0));
    let b2_hits = Arc::new(AtomicUsize::new(0));

    let l1 = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let a1 = l1.local_addr().unwrap();
    let b1_hits_srv = Arc::clone(&b1_hits);
    tokio::spawn(async move {
        while let Ok((mut s, _)) = l1.accept().await {
            let ok_flag = Arc::clone(&b1_ok_srv);
            let hits = Arc::clone(&b1_hits_srv);
            tokio::spawn(async move {
                let mut buf = [0u8; 1024];
                let n = s.read(&mut buf).await.unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);
                if req.starts_with("GET /health ") {
                    let body = if ok_flag.load(Ordering::Acquire) {
                        "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok"
                    } else {
                        "HTTP/1.1 503 Service Unavailable\r\nContent-Length: 3\r\nConnection: close\r\n\r\nbad"
                    };
                    let _ = s.write_all(body.as_bytes()).await;
                } else if req.starts_with("GET /slow ") {
                    tokio::time::sleep(Duration::from_millis(250)).await;
                } else {
                    hits.fetch_add(1, Ordering::Relaxed);
                    let _ = s
                        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nb1")
                        .await;
                }
            });
        }
    });

    let l2 = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let a2 = l2.local_addr().unwrap();
    let b2_hits_srv = Arc::clone(&b2_hits);
    tokio::spawn(async move {
        while let Ok((mut s, _)) = l2.accept().await {
            let hits = Arc::clone(&b2_hits_srv);
            tokio::spawn(async move {
                let mut buf = [0u8; 1024];
                let n = s.read(&mut buf).await.unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);
                if req.starts_with("GET /health ") {
                    let _ = s
                        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
                        .await;
                } else if req.starts_with("GET /slow ") {
                    tokio::time::sleep(Duration::from_millis(250)).await;
                } else {
                    hits.fetch_add(1, Ordering::Relaxed);
                    let _ = s
                        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nb2")
                        .await;
                }
            });
        }
    });

    let ctx = Context::new(FrameOptions::new(Arc::new(TokioSpawner)));
    ctx.plugin(TransportPlugin::default(), ConfigValue::Null)
        .unwrap();
    ctx.plugin(
        HttpEnginePlugin::new(HttpConfig {
            bind_addr: Some("127.0.0.1:0".parse().unwrap()),
            ..HttpConfig::default()
        }),
        ConfigValue::Null,
    )
    .unwrap();
    ctx.plugin(
        ProxyPlugin::new(ProxyConfig {
            upstreams: vec![UpstreamConfig {
                id: "pool".to_string(),
                strategy: LoadBalanceStrategy::RoundRobin,
                backends: vec![
                    BackendConfig::plain(a1),
                    BackendConfig::plain(a2),
                ],
                max_idle_per_backend: 0,
                connect_timeout_ms: 500,
                response_header_timeout_ms: 80,
                active_health_check: Some(HealthCheckConfig {
                    interval_ms: 25,
                    timeout_ms: 100,
                    path: Some("/health".to_string()),
                    unhealthy_threshold: 2,
                    healthy_threshold: 2,
                }),
                passive_eject_threshold: 0,
                passive_eject_ms: 500,
            }],
            http_routes: vec![HttpProxyRouteConfig {
                id: "all".to_string(),
                host: None,
                match_prefix: "/".to_string(),
                strip_prefix: None,
                preserve_host: false,
                upstream: "pool".to_string(),
                websocket: false,
                retry_idempotent: false,
                extra_headers: Vec::new(),
            }],
            tcp_routes: Vec::new(),
        }),
        ConfigValue::Null,
    )
    .unwrap();
    ctx.settle().await;

    let addr = ctx.get::<BoundServerInfo>().unwrap().local_addr;
    let pool = ctx.get::<UpstreamPoolService>().unwrap();

    // Wait for active health check to mark backend 1 unhealthy.
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let snap = pool.snapshot();
            if !snap[0].backends[0].healthy && snap[0].backends[1].healthy {
                break;
            }
            tokio::time::sleep(Duration::from_millis(15)).await;
        }
    })
    .await
    .expect("b1 must be ejected by active probe");

    for _ in 0..4 {
        let r = send_raw_http(
            addr,
            "GET /work HTTP/1.1\r\nHost: h\r\nConnection: close\r\n\r\n",
        )
        .await;
        assert!(r.ends_with("b2"), "expected healthy b2 only, got: {r}");
    }
    assert_eq!(b1_hits.load(Ordering::Relaxed), 0);
    assert_eq!(b2_hits.load(Ordering::Relaxed), 4);

    // Recover backend 1 and verify it re-enters rotation.
    b1_ok.store(true, Ordering::Release);
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let snap = pool.snapshot();
            if snap[0].backends[0].healthy {
                break;
            }
            tokio::time::sleep(Duration::from_millis(15)).await;
        }
    })
    .await
    .expect("b1 must recover");

    for _ in 0..4 {
        let _ = send_raw_http(
            addr,
            "GET /work HTTP/1.1\r\nHost: h\r\nConnection: close\r\n\r\n",
        )
        .await;
    }
    assert!(
        b1_hits.load(Ordering::Relaxed) >= 1,
        "recovered b1 must serve traffic again"
    );

    // GA14b: Slow upstream response header triggers 504 Gateway Timeout.
    let slow = send_raw_http(
        addr,
        "GET /slow HTTP/1.1\r\nHost: h\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        slow.starts_with("HTTP/1.1 504 "),
        "expected 504, got: {slow}"
    );

    ctx.dispose().await.unwrap();
}

#[tokio::test]
async fn stale_pooled_post_with_body_does_not_retry_empty_and_ipv6_tls_forwarded()
 {
    let upstream_ln = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_addr = upstream_ln.local_addr().unwrap();
    let second_conns = Arc::new(AtomicUsize::new(0));
    let second_conns_srv = Arc::clone(&second_conns);
    let captured = Arc::new(tokio::sync::Mutex::new(String::new()));
    let captured_srv = Arc::clone(&captured);

    tokio::spawn(async move {
        // Conn 1: serve GET /warm keep-alive, then immediately close socket to make pool stale.
        if let Ok((mut s1, _)) = upstream_ln.accept().await {
            let mut buf = [0u8; 1024];
            let _ = s1.read(&mut buf).await;
            let _ = s1
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: keep-alive\r\n\r\nwarm",
                )
                .await;
            drop(s1);
        }
        // Any subsequent fresh dials (for IPv6 TLS header test).
        while let Ok((mut s, _)) = upstream_ln.accept().await {
            second_conns_srv.fetch_add(1, Ordering::Relaxed);
            let mut buf = [0u8; 4096];
            if let Ok(n) = s.read(&mut buf).await {
                let text = String::from_utf8_lossy(&buf[..n]).into_owned();
                *captured_srv.lock().await = text;
            }
            let _ = s
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                )
                .await;
        }
    });

    let ctx = Context::new(FrameOptions::new(Arc::new(TokioSpawner)));
    ctx.plugin(
        TransportPlugin::new(TransportConfig {
            short_drain_timeout: Duration::from_millis(40),
            long_drain_timeout: Duration::from_millis(60),
            max_connections: None,
        }),
        ConfigValue::Null,
    )
    .unwrap();
    ctx.plugin(
        HttpEnginePlugin::new(HttpConfig {
            bind_addr: Some("127.0.0.1:0".parse().unwrap()),
            ..HttpConfig::default()
        }),
        ConfigValue::Null,
    )
    .unwrap();
    ctx.plugin(
        ProxyPlugin::new(ProxyConfig {
            upstreams: vec![UpstreamConfig {
                max_idle_per_backend: 4,
                ..UpstreamConfig::new(
                    "u",
                    vec![BackendConfig::plain(upstream_addr)],
                )
            }],
            http_routes: vec![HttpProxyRouteConfig {
                id: "all".to_string(),
                host: None,
                match_prefix: "/".to_string(),
                strip_prefix: None,
                preserve_host: false,
                upstream: "u".to_string(),
                websocket: false,
                retry_idempotent: true,
                extra_headers: Vec::new(),
            }],
            tcp_routes: Vec::new(),
        }),
        ConfigValue::Null,
    )
    .unwrap();
    ctx.settle().await;

    let router = ctx.get::<RouterService>().unwrap().as_ref().clone();

    // 1. Warm pool with GET /warm -> 200, backend then drops conn 1.
    let warm = router
        .dispatch(http_api::HttpRequest::new(http_api::Method::Get, "/warm"))
        .await;
    assert_eq!(warm.status, StatusCode::OK);
    tokio::time::sleep(Duration::from_millis(30)).await;

    // 2. Non-idempotent POST with non-empty Bytes body hits stale pooled socket:
    // must NOT silently retry with empty body -> fails cleanly with 502 and 0 extra dials!
    let mut post_req =
        http_api::HttpRequest::new(http_api::Method::Post, "/submit");
    post_req.body = http_api::BodyKind::Bytes(b"important=payload".to_vec());
    let post_resp = router.dispatch(post_req).await;
    assert_eq!(post_resp.status, StatusCode::BAD_GATEWAY);
    assert_eq!(
        second_conns.load(Ordering::Relaxed),
        0,
        "non-empty POST must never retry on a stale pooled socket with empty body"
    );

    // 3. IPv6 + is_tls=true request forwards quoted RFC 7239 Forwarded and https proto.
    let mut ipv6_tls =
        http_api::HttpRequest::new(http_api::Method::Get, "/probe");
    ipv6_tls.remote_addr = Some("[::1]:54321".parse().unwrap());
    ipv6_tls.is_tls = true;
    let tls_resp = router.dispatch(ipv6_tls).await;
    assert_eq!(tls_resp.status, StatusCode::OK);
    let raw_fwd = captured.lock().await.clone();
    assert!(
        raw_fwd.contains("Forwarded: for=\"[::1]\";proto=https\r\n"),
        "expected quoted RFC 7239 IPv6 Forwarded, got:\n{raw_fwd}"
    );
    assert!(
        raw_fwd.contains("X-Forwarded-Proto: https\r\n"),
        "expected X-Forwarded-Proto: https, got:\n{raw_fwd}"
    );

    ctx.dispose().await.unwrap();
}
