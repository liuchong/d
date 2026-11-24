//! BDD/TDD tests for Layer 2 HTTP/1.1 parser bounds, streaming chunk flush,
//! and snapshot router effect lifecycle (`T-H03`, `T-H04`, `T-H05`).

use frame::{ConfigValue, Context, FrameOptions};
use http_api::{
    FilterOutcome, HttpFilter, HttpRequest, HttpResponse, Method, RequestMeta,
    RouteMatch, RouteSpec, RouterService, StatusCode, stream_channel,
};
use l4_transport::{TokioSpawner, TransportPlugin};
use proto_http::{BoundServerInfo, HttpConfig, HttpEnginePlugin, fn_handler};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::Notify;

async fn setup_http_server() -> (Context, RouterService, std::net::SocketAddr) {
    let ctx = Context::new(FrameOptions::new(Arc::new(TokioSpawner)));
    ctx.plugin(TransportPlugin::default(), ConfigValue::Null)
        .expect("transport");
    ctx.plugin(
        HttpEnginePlugin::new(HttpConfig {
            bind_addr: Some("127.0.0.1:0".parse().unwrap()),
            ..HttpConfig::default()
        }),
        ConfigValue::Null,
    )
    .expect("http engine");
    ctx.settle().await;
    let router = ctx.get::<RouterService>().expect("router").as_ref().clone();
    let info = ctx.get::<BoundServerInfo>().expect("bound info");
    (ctx, router, info.local_addr)
}

async fn send_raw(addr: std::net::SocketAddr, req: &[u8]) -> Vec<u8> {
    let mut stream = TcpStream::connect(addr).await.expect("connect");
    stream.write_all(req).await.expect("write");
    let mut out = Vec::new();
    stream.read_to_end(&mut out).await.expect("read");
    out
}

#[tokio::test]
async fn parser_keep_alive_chunked_and_malformed_defense() {
    let (ctx, router, addr) = setup_http_server().await;

    let _a = router
        .mount_route(
            RouteSpec::new("a", RouteMatch::Exact("/a".to_string())),
            fn_handler(|_req| async {
                HttpResponse::text(StatusCode::OK, "alpha")
            }),
        )
        .expect("mount /a");

    let _b = router
        .mount_route(
            RouteSpec::new("b", RouteMatch::Exact("/b".to_string())),
            fn_handler(|req| async move {
                let bytes = req.body.into_bytes().await.unwrap_or_default();
                HttpResponse::bytes(
                    StatusCode::OK,
                    "text/plain; charset=utf-8",
                    bytes,
                )
            }),
        )
        .expect("mount /b");

    // 1. Two sequential pipelined requests over one keep-alive socket + chunked POST body.
    let mut stream = TcpStream::connect(addr).await.expect("connect");
    let pipelined = b"GET /a HTTP/1.1\r\nHost: localhost\r\n\r\nPOST /b HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n5\r\nhello\r\n0\r\n\r\n";
    stream.write_all(pipelined).await.expect("write pipelined");
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).await.expect("read pipelined");
    let text = String::from_utf8_lossy(&raw);
    assert!(
        text.contains("HTTP/1.1 200 OK")
            && text.contains("alpha")
            && text.contains("hello"),
        "unexpected pipelined output: {text}"
    );
    let alpha_pos = text.find("alpha").expect("alpha present");
    let hello_pos = text.find("hello").expect("hello present");
    assert!(alpha_pos < hello_pos, "pipelined ordering broken: {text}");

    // 2. Request line > 8 KB -> 414 URI Too Long.
    let long_path = format!("/{}", "x".repeat(9000));
    let req_414 = format!(
        "GET {long_path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
    );
    let resp_414 =
        String::from_utf8_lossy(&send_raw(addr, req_414.as_bytes()).await)
            .into_owned();
    assert!(
        resp_414.starts_with("HTTP/1.1 414 "),
        "expected 414, got: {resp_414}"
    );

    // 3. Headers > 64 KB -> 431 Request Header Fields Too Large.
    let huge_val = "v".repeat(70 * 1024);
    let req_431 = format!(
        "GET /a HTTP/1.1\r\nHost: localhost\r\nX-Big: {huge_val}\r\nConnection: close\r\n\r\n"
    );
    let resp_431 =
        String::from_utf8_lossy(&send_raw(addr, req_431.as_bytes()).await)
            .into_owned();
    assert!(
        resp_431.starts_with("HTTP/1.1 431 "),
        "expected 431, got: {resp_431}"
    );

    // 4. Unsupported HTTP version -> 505.
    let resp_505 = String::from_utf8_lossy(
        &send_raw(addr, b"GET /a HTTP/0.9\r\n\r\n").await,
    )
    .into_owned();
    assert!(
        resp_505.starts_with("HTTP/1.1 505 "),
        "expected 505, got: {resp_505}"
    );

    // 5. Malformed request line -> 400.
    let resp_400 =
        String::from_utf8_lossy(&send_raw(addr, b"GARBAGE\r\n\r\n").await)
            .into_owned();
    assert!(
        resp_400.starts_with("HTTP/1.1 400 "),
        "expected 400, got: {resp_400}"
    );

    ctx.dispose().await.expect("dispose");
}

#[tokio::test]
async fn streaming_response_flushes_chunks_before_close() {
    let (ctx, router, addr) = setup_http_server().await;
    let ack = Arc::new(Notify::new());
    let ack_route = Arc::clone(&ack);

    let _sse = router
        .mount_route(
            RouteSpec::new("sse", RouteMatch::Exact("/sse".to_string())),
            fn_handler(move |_req| {
                let ack = Arc::clone(&ack_route);
                async move {
                    let (tx, rx) = stream_channel();
                    tokio::spawn(async move {
                        let _ = tx.send(b"data: chunk1\n\n".to_vec());
                        ack.notified().await;
                        let _ = tx.send(b"data: chunk2\n\n".to_vec());
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
        .expect("mount sse");

    let mut stream = TcpStream::connect(addr).await.expect("connect");
    stream
        .write_all(b"GET /sse HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .expect("write");

    let mut received = String::new();
    let mut buf = [0u8; 512];
    let deadline =
        tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    while !received.contains("data: chunk1\n\n") {
        let remaining =
            deadline.saturating_duration_since(tokio::time::Instant::now());
        assert!(
            !remaining.is_zero(),
            "timed out waiting for chunk1: {received}"
        );
        let n = tokio::time::timeout(remaining, stream.read(&mut buf))
            .await
            .expect("chunk1 arrival")
            .expect("tcp read");
        assert!(n > 0, "EOF before chunk1");
        received.push_str(&String::from_utf8_lossy(&buf[..n]));
    }

    assert!(
        !received.contains("data: chunk2"),
        "chunk2 should still be gated behind ack"
    );
    ack.notify_one();

    let mut tail = Vec::new();
    stream.read_to_end(&mut tail).await.expect("read rest");
    received.push_str(&String::from_utf8_lossy(&tail));
    assert!(
        received.contains("data: chunk2\n\n"),
        "missing chunk2: {received}"
    );

    ctx.dispose().await.expect("dispose");
}

struct DenyFilter {
    prefix: String,
}

impl HttpFilter for DenyFilter {
    fn name(&self) -> &str {
        "deny-filter"
    }

    fn priority(&self) -> i32 {
        -10
    }

    fn on_request(
        &self,
        req: HttpRequest,
    ) -> frame::BoxFuture<'static, FilterOutcome> {
        let block = req.path.starts_with(&self.prefix);
        Box::pin(async move {
            if block {
                FilterOutcome::Respond(HttpResponse::text(
                    StatusCode::FORBIDDEN,
                    "blocked",
                ))
            } else {
                FilterOutcome::Continue(req)
            }
        })
    }

    fn on_response(
        &self,
        _req_meta: &RequestMeta,
        resp: HttpResponse,
    ) -> frame::BoxFuture<'static, HttpResponse> {
        Box::pin(async { resp })
    }
}

#[tokio::test]
async fn router_effect_lifecycle_and_snapshot_isolation() {
    let ctx = Context::new(FrameOptions::new(Arc::new(TokioSpawner)));
    ctx.plugin(HttpEnginePlugin::default(), ConfigValue::Null)
        .expect("http engine");
    ctx.settle().await;
    let router = ctx.get::<RouterService>().expect("router").as_ref().clone();

    // Missing route -> 404
    let r0 = router.dispatch(HttpRequest::new(Method::Get, "/dyn")).await;
    assert_eq!(r0.status, StatusCode::NOT_FOUND);

    // Filter short-circuit + disposal
    let filter_h = router
        .mount_filter(Arc::new(DenyFilter {
            prefix: "/secret".to_string(),
        }))
        .expect("mount filter");
    let blocked = router
        .dispatch(HttpRequest::new(Method::Get, "/secret/x"))
        .await;
    assert_eq!(blocked.status, StatusCode::FORBIDDEN);
    filter_h.dispose().await.expect("dispose filter");

    // Dynamic mount & dispose across 200 concurrent snapshot updates
    let shared_router = router.clone();
    let reader_task = tokio::spawn(async move {
        for _ in 0..200 {
            let resp = shared_router
                .dispatch(HttpRequest::new(Method::Get, "/dyn"))
                .await;
            assert!(
                resp.status == StatusCode::OK
                    || resp.status == StatusCode::NOT_FOUND,
                "unexpected torn status: {}",
                resp.status
            );
        }
    });

    for _ in 0..200 {
        let route_h = router
            .mount_route(
                RouteSpec::new("dyn", RouteMatch::Exact("/dyn".to_string())),
                fn_handler(|_req| async {
                    HttpResponse::text(StatusCode::OK, "ok")
                }),
            )
            .expect("mount dyn");
        route_h.dispose().await.expect("dispose dyn");
    }

    reader_task.await.expect("reader");
    let after = router.dispatch(HttpRequest::new(Method::Get, "/dyn")).await;
    assert_eq!(after.status, StatusCode::NOT_FOUND);

    ctx.dispose().await.expect("dispose");
}
