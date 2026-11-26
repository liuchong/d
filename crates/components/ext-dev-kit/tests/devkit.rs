//! Integration tests for `ext-dev-kit` (`GA18`–`GA19`):
//! static & SSE Mocks, Live Reload watcher WS push + inline HTML injection,
//! and white `#ffffff` zero-CDN `/_d/inspect` SSE timeline + replay.

use d_ext_dev_kit::livereload::LiveReloadConfig;
use d_ext_dev_kit::mock::{MockRule, MockSseChunk};
use d_ext_dev_kit::{DevKitConfig, DevKitPlugin};
use frame::{ConfigValue, Context, FrameOptions};
use http_api::{BodyKind, HttpRequest, Method, RouterService, StatusCode};
use l4_transport::{TokioSpawner, TransportConfig, TransportPlugin};
use proto_http::{BoundServerInfo, HttpConfig, HttpEnginePlugin};
use proto_ws::WsEnginePlugin;
use proto_ws::frame::decode_frame;
use std::sync::Arc;
use std::time::Duration;
use tempfile::tempdir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

#[tokio::test]
async fn dev_kit_mocks_sse_inspector_replay_and_live_reload_work() {
    let watch_dir = tempdir().unwrap();
    std::fs::write(watch_dir.path().join("app.css"), "body{}").unwrap();

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
    ctx.plugin(WsEnginePlugin::default(), ConfigValue::Null)
        .unwrap();
    ctx.plugin(
        DevKitPlugin::new(DevKitConfig {
            mocks: vec![
                MockRule {
                    id: "page".to_string(),
                    method: Method::Get,
                    path: "/demo.html".to_string(),
                    status: 200,
                    headers: vec![(
                        "Content-Type".to_string(),
                        "text/html; charset=utf-8".to_string(),
                    )],
                    body: "<html><body><h1>Hi</h1></body></html>".to_string(),
                    sse_chunks: Vec::new(),
                },
                MockRule {
                    id: "ai-sse".to_string(),
                    method: Method::Get,
                    path: "/mock/sse".to_string(),
                    status: 200,
                    headers: Vec::new(),
                    body: String::new(),
                    sse_chunks: vec![
                        MockSseChunk {
                            data: "data: tok1\n\n".to_string(),
                            delay_ms: 0,
                        },
                        MockSseChunk {
                            data: "data: tok2\n\n".to_string(),
                            delay_ms: 15,
                        },
                    ],
                },
            ],
            live_reload: LiveReloadConfig {
                enabled: true,
                watch_dirs: vec![watch_dir.path().to_path_buf()],
                interval_ms: 30,
                ws_path: "/_d/livereload".to_string(),
                inject_html: true,
            },
            inspect_enabled: true,
            inspect_path: "/_d/inspect".to_string(),
            inspect_max_entries: 50,
        }),
        ConfigValue::Null,
    )
    .unwrap();
    ctx.settle().await;

    let router = ctx.get::<RouterService>().unwrap().as_ref().clone();
    let addr = ctx.get::<BoundServerInfo>().unwrap().local_addr;

    // 1. Mock HTML injects local offline WebSocket reload script without external CDNs.
    let page = router
        .dispatch(HttpRequest::new(Method::Get, "/demo.html"))
        .await;
    assert_eq!(page.status, StatusCode::OK);
    let page_html =
        String::from_utf8(page.body.into_bytes().await.unwrap()).unwrap();
    assert!(page_html.contains("/_d/livereload"));
    assert!(!page_html.contains("http://") && !page_html.contains("https://"));

    // 2. Mock SSE emits streaming body tapped by InspectStreamTap.
    let sse = router
        .dispatch(HttpRequest::new(Method::Get, "/mock/sse"))
        .await;
    assert!(matches!(
        sse.body,
        BodyKind::Stream {
            streaming: true,
            ..
        }
    ));
    let sse_bytes = sse.body.into_bytes().await.unwrap();
    assert_eq!(
        String::from_utf8(sse_bytes).unwrap(),
        "data: tok1\n\ndata: tok2\n\n"
    );

    // 3. White #ffffff /_d/inspect UI & JSON API record SSE chunks + replay works.
    let ui = router
        .dispatch(HttpRequest::new(Method::Get, "/_d/inspect"))
        .await;
    let ui_html =
        String::from_utf8(ui.body.into_bytes().await.unwrap()).unwrap();
    assert!(ui_html.contains("background:#ffffff"));
    assert!(!ui_html.contains("https://"));
    assert!(ui_html.contains("tok1"));

    let events = router
        .dispatch(HttpRequest::new(Method::Get, "/_d/inspect/api/events"))
        .await;
    let events_json =
        String::from_utf8(events.body.into_bytes().await.unwrap()).unwrap();
    assert!(
        events_json.contains("\"path\":\"/mock/sse\""),
        "{events_json}"
    );
    assert!(events_json.contains("\"sse_chunks\":[{"), "{events_json}");

    let replay = router
        .dispatch(HttpRequest::new(
            Method::Post,
            "/_d/inspect/api/replay?id=1",
        ))
        .await;
    assert_eq!(replay.status, StatusCode::OK);

    // 4. Connect to /_d/livereload over WebSocket, modify watched file, observe push frame.
    let mut ws = TcpStream::connect(addr).await.unwrap();
    ws.write_all(
        b"GET /_d/livereload HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n",
    )
    .await
    .unwrap();
    let mut buf = Vec::new();
    let mut tmp = [0u8; 512];
    loop {
        let n = ws.read(&mut tmp).await.unwrap();
        assert!(n > 0);
        buf.extend_from_slice(&tmp[..n]);
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            buf.drain(..pos + 4);
            break;
        }
    }

    tokio::time::sleep(Duration::from_millis(60)).await;
    std::fs::write(watch_dir.path().join("app.css"), "body{color:#111;}")
        .unwrap();

    let got_reload = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if let Ok((f, used)) = decode_frame(&buf, 65536) {
                buf.drain(..used);
                let txt = String::from_utf8_lossy(&f.payload);
                if txt.contains("reload") {
                    return true;
                }
            }
            let n = ws.read(&mut tmp).await.unwrap();
            if n == 0 {
                return false;
            }
            buf.extend_from_slice(&tmp[..n]);
        }
    })
    .await
    .expect("live reload frame timeout");
    assert!(got_reload);

    ctx.dispose().await.unwrap();
}
