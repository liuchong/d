//! Integration tests for Layer 2 raw TCP engine (`proto-tcp`):
//! raw handler registration, bidirectional forwarding rules, and clean port release.

use d_proto_tcp::{
    TcpConfig, TcpEnginePlugin, TcpForwardRule, TcpRegistryService,
};
use frame::{ConfigValue, Context, ErrorKind, FrameOptions, Plugin};
use l4_transport::{TokioSpawner, TransportConfig, TransportPlugin};
use net_api::{
    BoxStreamIo, ConnectionHandler, ConnectionMeta, DrainSignal, ListenOptions,
};
use std::collections::BTreeMap;
use std::future::poll_fn;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

struct PrefixHandler;

impl ConnectionHandler for PrefixHandler {
    fn handle(
        &self,
        mut io: BoxStreamIo,
        _meta: ConnectionMeta,
        _drain: Arc<dyn DrainSignal>,
    ) -> frame::BoxFuture<'static, ()> {
        Box::pin(async move {
            let mut buf = [0u8; 128];
            if let Ok(n) =
                poll_fn(|cx| Pin::new(&mut *io).poll_read(cx, &mut buf)).await
                && n > 0
            {
                let mut reply = b"RAW:".to_vec();
                reply.extend_from_slice(&buf[..n]);
                let _ = poll_fn(|cx| Pin::new(&mut *io).poll_write(cx, &reply))
                    .await;
                let _ = poll_fn(|cx| Pin::new(&mut *io).poll_flush(cx)).await;
            }
            let _ = poll_fn(|cx| Pin::new(&mut *io).poll_shutdown(cx)).await;
        })
    }
}

#[tokio::test]
async fn tcp_registry_and_declarative_forwarding_work_and_cleanup() {
    // Config validation check
    let bad = ConfigValue::Map(BTreeMap::from([(
        "rules".to_string(),
        ConfigValue::Str("invalid".to_string()),
    )]));
    assert_eq!(
        TcpEnginePlugin::default()
            .validate_config(&bad)
            .expect_err("bad rules must reject")
            .kind,
        ErrorKind::ValidationFailed
    );

    // Backend echo server for declarative forwarding rule
    let backend = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let backend_addr = backend.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = backend.accept().await {
            tokio::spawn(async move {
                let mut b = [0u8; 64];
                if let Ok(n) = s.read(&mut b).await
                    && n > 0
                {
                    let _ = s.write_all(&b[..n]).await;
                    let _ = s.shutdown().await;
                }
            });
        }
    });

    // Reserve a free port for declarative listen_addr
    let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let fwd_addr: SocketAddr = probe.local_addr().unwrap();
    drop(probe);

    let ctx = Context::new(FrameOptions::new(Arc::new(TokioSpawner)));
    ctx.plugin(
        TransportPlugin::new(TransportConfig {
            short_drain_timeout: Duration::from_millis(40),
            long_drain_timeout: Duration::from_millis(60),
            max_connections: None,
        }),
        ConfigValue::Null,
    )
    .expect("transport");
    ctx.plugin(
        TcpEnginePlugin::new(TcpConfig {
            rules: vec![TcpForwardRule {
                id: "fwd".to_string(),
                listen_addr: fwd_addr,
                target_addr: backend_addr,
                max_connections: Some(8),
                tls_listener_id: None,
            }],
        }),
        ConfigValue::Null,
    )
    .expect("proto-tcp");
    ctx.settle().await;

    // 1. Test declarative forwarding
    let mut fwd_conn = TcpStream::connect(fwd_addr).await.expect("connect fwd");
    fwd_conn.write_all(b"ping-tcp").await.unwrap();
    let mut fwd_out = Vec::new();
    fwd_conn.read_to_end(&mut fwd_out).await.unwrap();
    assert_eq!(&fwd_out, b"ping-tcp");

    // 2. Test imperative ScopedTcpRegistry custom protocol mounting
    let tcp = ctx.get::<TcpRegistryService>().expect("tcp svc");
    let (raw_addr, handle) = tcp
        .mount_tcp(
            &ctx,
            ListenOptions::new("127.0.0.1:0".parse().unwrap()),
            Arc::new(PrefixHandler),
        )
        .await
        .expect("mount raw tcp");
    let mut raw_conn = TcpStream::connect(raw_addr).await.unwrap();
    raw_conn.write_all(b"hello").await.unwrap();
    let mut raw_out = Vec::new();
    raw_conn.read_to_end(&mut raw_out).await.unwrap();
    assert_eq!(&raw_out, b"RAW:hello");

    handle.dispose().await.expect("dispose raw");
    assert!(TcpStream::connect(raw_addr).await.is_err());

    ctx.dispose().await.expect("dispose");
}
