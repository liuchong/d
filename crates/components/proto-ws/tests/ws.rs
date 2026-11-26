//! Integration tests for pure-Rust RFC 6455 WebSocket engine (`proto-ws`):
//! handshake errors (400/426), fragmented text/binary frames, Ping->Pong,
//! unmasked client frame close(1002), backpressure try_send, and Close(1001) drain.

use d_proto_ws::frame::{decode_frame, encode_frame};
use d_proto_ws::handshake::compute_accept_key;
use d_proto_ws::{WsConfig, WsEnginePlugin};
use frame::{ConfigValue, Context, FrameOptions};
use http_api::{RouteMatch, RouteSpec, RouterService, StatusCode};
use l4_transport::{TokioSpawner, TransportConfig, TransportPlugin};
use proto_http::{BoundServerInfo, HttpConfig, HttpEnginePlugin};
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::Notify;
use ws_api::{WsHandler, WsMessage, WsOpcode, WsRegistryService, WsSession};

struct EchoWsHandler {
    backpressure_checked: Arc<Notify>,
    trigger_backpressure: bool,
}

impl WsHandler for EchoWsHandler {
    fn on_connect(&self, session: WsSession) -> frame::BoxFuture<'static, ()> {
        let bp = self.trigger_backpressure;
        let bp_notify = Arc::clone(&self.backpressure_checked);
        Box::pin(async move {
            if bp {
                // Fill bounded send_queue_capacity=2 until WouldBlock appears.
                let mut hit_would_block = false;
                for i in 0..20 {
                    match session
                        .try_send(WsMessage::Text(format!("burst-{i}")))
                    {
                        Ok(()) => {}
                        Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                            hit_would_block = true;
                            break;
                        }
                        Err(_) => break,
                    }
                }
                assert!(
                    hit_would_block,
                    "bounded WS queue must trigger WouldBlock"
                );
                bp_notify.notify_one();
            }

            while let Some(item) = session.recv().await {
                match item {
                    Ok(WsMessage::Text(txt)) => {
                        let _ = session
                            .send(WsMessage::Text(format!(
                                "echo:{txt}:{}",
                                session
                                    .path_params()
                                    .get("room")
                                    .map_or("", String::as_str)
                            )))
                            .await;
                    }
                    Ok(WsMessage::Binary(bin)) => {
                        let _ = session.send(WsMessage::Binary(bin)).await;
                    }
                    Ok(WsMessage::Close(_)) | Err(_) => break,
                    Ok(_) => {}
                }
            }
        })
    }
}

async fn setup_ws(
    send_queue_capacity: usize,
) -> (Context, WsRegistryService, RouterService, SocketAddr) {
    let ctx = Context::new(FrameOptions::new(Arc::new(TokioSpawner)));
    ctx.plugin(
        TransportPlugin::new(TransportConfig {
            short_drain_timeout: Duration::from_millis(50),
            long_drain_timeout: Duration::from_millis(80),
            max_connections: None,
        }),
        ConfigValue::Null,
    )
    .expect("transport");
    ctx.plugin(
        HttpEnginePlugin::new(HttpConfig {
            bind_addr: Some("127.0.0.1:0".parse().unwrap()),
            ..HttpConfig::default()
        }),
        ConfigValue::Null,
    )
    .expect("http");
    ctx.plugin(
        WsEnginePlugin::new(WsConfig {
            send_queue_capacity,
            ..WsConfig::default()
        }),
        ConfigValue::Null,
    )
    .expect("ws");
    ctx.settle().await;
    let ws = ctx
        .get::<WsRegistryService>()
        .expect("ws svc")
        .as_ref()
        .clone();
    let router = ctx.get::<RouterService>().expect("router").as_ref().clone();
    let addr = ctx.get::<BoundServerInfo>().expect("addr").local_addr;
    (ctx, ws, router, addr)
}

async fn read_http_head(stream: &mut TcpStream) -> (String, Vec<u8>) {
    let mut raw = Vec::new();
    let mut tmp = [0u8; 512];
    loop {
        let n = stream.read(&mut tmp).await.expect("read head");
        assert!(n > 0, "eof waiting for HTTP head");
        raw.extend_from_slice(&tmp[..n]);
        if let Some(pos) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&raw[..pos + 4]).into_owned();
            let leftover = raw[pos + 4..].to_vec();
            return (head, leftover);
        }
    }
}

#[tokio::test]
async fn websocket_handshake_fragmentation_control_frames_and_unmasked_rejection()
 {
    let (ctx, ws, _router, addr) = setup_ws(32).await;
    let _route = ws
        .mount_ws(
            &ctx,
            RouteSpec::new("chat", RouteMatch::Param("/ws/:room".to_string())),
            Arc::new(EchoWsHandler {
                backpressure_checked: Arc::new(Notify::new()),
                trigger_backpressure: false,
            }),
        )
        .expect("mount ws");

    // 1. Bad version -> 426
    let mut bad_ver = TcpStream::connect(addr).await.unwrap();
    bad_ver
        .write_all(
            b"GET /ws/lobby HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 8\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n",
        )
        .await
        .unwrap();
    let (h, _) = read_http_head(&mut bad_ver).await;
    assert!(
        h.starts_with(&format!(
            "HTTP/1.1 {} ",
            StatusCode::UPGRADE_REQUIRED.as_u16()
        )),
        "{h}"
    );
    drop(bad_ver);

    // 2. Valid RFC 6455 upgrade -> 101 + fragmented Text + Binary + Ping->Pong + Close
    let mut client = TcpStream::connect(addr).await.unwrap();
    client
        .write_all(
            b"GET /ws/lobby HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n",
        )
        .await
        .unwrap();
    let (head, mut rx_buf) = read_http_head(&mut client).await;
    assert!(
        head.starts_with("HTTP/1.1 101 Switching Protocols"),
        "{head}"
    );
    assert!(
        head.contains(&format!(
            "Sec-WebSocket-Accept: {}",
            compute_accept_key("dGhlIHNhbXBsZSBub25jZQ==")
        )),
        "{head}"
    );

    let mask = [0x11, 0x22, 0x33, 0x44];
    // Send fragmented Text ("hel" + "lo")
    client
        .write_all(&encode_frame(false, WsOpcode::Text, b"hel", Some(mask)))
        .await
        .unwrap();
    client
        .write_all(&encode_frame(
            true,
            WsOpcode::Continuation,
            b"lo",
            Some(mask),
        ))
        .await
        .unwrap();
    // Send Ping
    client
        .write_all(&encode_frame(true, WsOpcode::Ping, b"hb", Some(mask)))
        .await
        .unwrap();
    // Send Binary
    client
        .write_all(&encode_frame(
            true,
            WsOpcode::Binary,
            &[1, 2, 3],
            Some(mask),
        ))
        .await
        .unwrap();

    let mut frames = Vec::new();
    let mut tmp = [0u8; 512];
    while frames.len() < 3 {
        while let Ok((f, used)) = decode_frame(&rx_buf, 65536) {
            rx_buf.drain(..used);
            frames.push(f);
        }
        if frames.len() >= 3 {
            break;
        }
        let n = client.read(&mut tmp).await.unwrap();
        assert!(n > 0, "eof before 3 frames arrived");
        rx_buf.extend_from_slice(&tmp[..n]);
    }
    assert!(frames.iter().any(
        |f| f.opcode == WsOpcode::Text && f.payload == b"echo:hello:lobby"
    ));
    assert!(
        frames
            .iter()
            .any(|f| f.opcode == WsOpcode::Pong && f.payload == b"hb")
    );
    assert!(
        frames
            .iter()
            .any(|f| f.opcode == WsOpcode::Binary && f.payload == [1, 2, 3])
    );

    // 3. Unmasked client frame triggers Close(1002).
    client
        .write_all(&encode_frame(true, WsOpcode::Text, b"bad", None))
        .await
        .unwrap();
    let mut got_1002 = false;
    loop {
        while let Ok((f, used)) = decode_frame(&rx_buf, 65536) {
            rx_buf.drain(..used);
            if f.opcode == WsOpcode::Close && f.payload.len() >= 2 {
                let code = u16::from_be_bytes([f.payload[0], f.payload[1]]);
                if code == 1002 {
                    got_1002 = true;
                }
            }
        }
        if got_1002 {
            break;
        }
        let n = client.read(&mut tmp).await.unwrap();
        if n == 0 {
            break;
        }
        rx_buf.extend_from_slice(&tmp[..n]);
    }
    assert!(got_1002, "unmasked frame must close with 1002");

    ctx.dispose().await.expect("dispose");
}

#[tokio::test]
async fn websocket_backpressure_and_graceful_drain_close_1001() {
    let (ctx, ws, _router, addr) = setup_ws(2).await;
    let bp_ready = Arc::new(Notify::new());
    let _route = ws
        .mount_ws(
            &ctx,
            RouteSpec::new("bp", RouteMatch::Exact("/bp".to_string())),
            Arc::new(EchoWsHandler {
                backpressure_checked: Arc::clone(&bp_ready),
                trigger_backpressure: true,
            }),
        )
        .expect("mount ws");

    let mut client = TcpStream::connect(addr).await.unwrap();
    client
        .write_all(
            b"GET /bp HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n",
        )
        .await
        .unwrap();
    let (_head, mut rx_buf) = read_http_head(&mut client).await;
    bp_ready.notified().await;

    // Trigger graceful drain -> expects Close(1001).
    let dispose_task = tokio::spawn(async move { ctx.dispose().await });
    let mut got_1001 = false;
    let mut tmp = [0u8; 512];
    loop {
        while let Ok((f, used)) = decode_frame(&rx_buf, 65536) {
            rx_buf.drain(..used);
            if f.opcode == WsOpcode::Close && f.payload.len() >= 2 {
                let code = u16::from_be_bytes([f.payload[0], f.payload[1]]);
                if code == 1001 {
                    got_1001 = true;
                }
            }
        }
        if got_1001 {
            break;
        }
        let n = client.read(&mut tmp).await.unwrap();
        if n == 0 {
            break;
        }
        rx_buf.extend_from_slice(&tmp[..n]);
    }
    assert!(got_1001, "graceful drain must deliver Close(1001)");
    drop(client);
    dispose_task.await.unwrap().expect("dispose");
}
