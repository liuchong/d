//! Integration tests for Layer 1 pure-Rust TLS (`l4-tls`):
//! SNI selection, ALPN negotiation, two-phase drain with TLS, invalid PEM preflight,
//! and atomic hot cert reload without interrupting established TLS sessions.

use frame::{
    ConfigValue, Context, EntryTarget, ErrorKind, FrameOptions, Loader, Plugin,
    PluginEntry, StaticResolver,
};
use l4_tls::TlsPlugin;
use l4_transport::{TokioSpawner, TransportConfig, TransportPlugin};
use net_api::{
    BoxStreamIo, ConnectionHandler, ConnectionMeta, DrainSignal, ListenOptions,
    TransportService,
};
use std::collections::BTreeMap;
use std::future::poll_fn;
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tls_api::{TlsClientConfig, TlsConnectorService};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio::sync::{Notify, oneshot};

fn gen_cert(host: &str) -> (String, String) {
    let ck = rcgen::generate_simple_self_signed(vec![host.to_string()])
        .expect("generate self-signed cert");
    (ck.cert.pem(), ck.signing_key.serialize_pem())
}

fn tls_entry_config(
    certs: Vec<(Option<&str>, &str, &str)>,
    alpn: &[&str],
) -> ConfigValue {
    let cert_list = certs
        .into_iter()
        .map(|(sni, cert, key)| {
            let mut m = BTreeMap::new();
            m.insert(
                "server_name".to_string(),
                sni.map_or(ConfigValue::Null, |s| {
                    ConfigValue::Str(s.to_string())
                }),
            );
            m.insert(
                "cert_pem".to_string(),
                ConfigValue::Str(cert.to_string()),
            );
            m.insert("key_pem".to_string(), ConfigValue::Str(key.to_string()));
            ConfigValue::Map(m)
        })
        .collect();

    let mut listener = BTreeMap::new();
    listener.insert("id".to_string(), ConfigValue::Str("https".to_string()));
    listener.insert(
        "alpn".to_string(),
        ConfigValue::List(
            alpn.iter()
                .map(|p| ConfigValue::Str((*p).to_string()))
                .collect(),
        ),
    );
    listener.insert("certs".to_string(), ConfigValue::List(cert_list));

    ConfigValue::Map(BTreeMap::from([(
        "listeners".to_string(),
        ConfigValue::List(vec![ConfigValue::Map(listener)]),
    )]))
}

struct TlsIoTcp(TcpStream);

impl net_api::AsyncByteIo for TlsIoTcp {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut [u8],
    ) -> std::task::Poll<io::Result<usize>> {
        let mut rbuf = tokio::io::ReadBuf::new(buf);
        match Pin::new(&mut self.0).poll_read(cx, &mut rbuf) {
            std::task::Poll::Ready(Ok(())) => {
                std::task::Poll::Ready(Ok(rbuf.filled().len()))
            }
            std::task::Poll::Ready(Err(e)) => std::task::Poll::Ready(Err(e)),
            std::task::Poll::Pending => std::task::Poll::Pending,
        }
    }

    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<io::Result<usize>> {
        Pin::new(&mut self.0).poll_write(cx, buf)
    }

    fn poll_flush(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_shutdown(cx)
    }
}

async fn write_stream(io: &mut BoxStreamIo, data: &[u8]) -> io::Result<()> {
    let mut off = 0;
    while off < data.len() {
        let n = poll_fn(|cx| Pin::new(&mut **io).poll_write(cx, &data[off..]))
            .await?;
        off += n;
    }
    poll_fn(|cx| Pin::new(&mut **io).poll_flush(cx)).await
}

async fn read_chunk(io: &mut BoxStreamIo) -> io::Result<Vec<u8>> {
    let mut buf = [0u8; 256];
    let n = poll_fn(|cx| Pin::new(&mut **io).poll_read(cx, &mut buf)).await?;
    Ok(buf[..n].to_vec())
}

struct EchoAndHoldHandler {
    seen_meta: Arc<Mutex<Vec<ConnectionMeta>>>,
    drain_notified: Arc<Notify>,
}

impl ConnectionHandler for EchoAndHoldHandler {
    fn handle(
        &self,
        mut io: BoxStreamIo,
        meta: ConnectionMeta,
        drain: Arc<dyn DrainSignal>,
    ) -> frame::BoxFuture<'static, ()> {
        let seen_meta = Arc::clone(&self.seen_meta);
        let drain_notified = Arc::clone(&self.drain_notified);
        Box::pin(async move {
            seen_meta.lock().expect("meta").push(meta);
            let Ok(first) = read_chunk(&mut io).await else {
                return;
            };
            if first == b"HOLD" {
                drain.mark_long_lived();
                let _ = write_stream(&mut io, b"HELD").await;
                let drain_fut = drain.wait();
                tokio::pin!(drain_fut);
                loop {
                    let mut buf = [0u8; 64];
                    tokio::select! {
                        biased;
                        _ = &mut drain_fut => {
                            drain_notified.notify_one();
                            let _ = write_stream(&mut io, b"BYE").await;
                            let _ = poll_fn(|cx| Pin::new(&mut *io).poll_shutdown(cx)).await;
                            return;
                        }
                        res = poll_fn(|cx| Pin::new(&mut *io).poll_read(cx, &mut buf)) => {
                            match res {
                                Ok(0) | Err(_) => return,
                                Ok(n) => {
                                    if write_stream(&mut io, &buf[..n]).await.is_err() {
                                        return;
                                    }
                                }
                            }
                        }
                    }
                }
            } else {
                let _ = write_stream(&mut io, &first).await;
                let _ =
                    poll_fn(|cx| Pin::new(&mut *io).poll_shutdown(cx)).await;
            }
        })
    }
}

#[tokio::test]
async fn tls_sni_alpn_preflight_hot_reload_and_drain_work() {
    // GA04 part 1: Invalid PEM fails validate_config preflight.
    let bad_cfg = tls_entry_config(
        vec![(Some("bad.test"), "not-a-cert", "not-a-key")],
        &["http/1.1"],
    );
    let err = TlsPlugin::default()
        .validate_config(&bad_cfg)
        .expect_err("invalid PEM must fail preflight");
    assert_eq!(err.kind, ErrorKind::ValidationFailed);

    let (cert_a, key_a) = gen_cert("a.test");
    let (cert_b, key_b) = gen_cert("b.test");
    let (cert_a2, key_a2) = gen_cert("a.test");

    let ctx = Context::new(FrameOptions::new(Arc::new(TokioSpawner)));
    let resolver = Arc::new(
        StaticResolver::new()
            .register(
                "l4-transport",
                TransportPlugin::new(TransportConfig {
                    short_drain_timeout: Duration::from_millis(80),
                    long_drain_timeout: Duration::from_millis(120),
                    max_connections: None,
                }),
            )
            .register("l4-tls", TlsPlugin::default()),
    );
    let loader = Loader::new(ctx.clone(), resolver);
    loader
        .apply(vec![
            PluginEntry {
                id: "l4-transport".to_string(),
                target: EntryTarget::Name("l4-transport".to_string()),
                config: ConfigValue::Null,
                disabled: false,
                isolate: Vec::new(),
                children: Vec::new(),
            },
            PluginEntry {
                id: "l4-tls".to_string(),
                target: EntryTarget::Name("l4-tls".to_string()),
                config: tls_entry_config(
                    vec![
                        (Some("a.test"), &cert_a, &key_a),
                        (Some("b.test"), &cert_b, &key_b),
                    ],
                    &["http/1.1"],
                ),
                disabled: false,
                isolate: Vec::new(),
                children: Vec::new(),
            },
        ])
        .await
        .expect("initial tls apply");
    ctx.settle().await;

    let transport = ctx.get::<TransportService>().expect("transport");
    let connector = ctx.get::<TlsConnectorService>().expect("connector");
    let seen_meta = Arc::new(Mutex::new(Vec::new()));
    let drain_notified = Arc::new(Notify::new());

    let (addr, listener_handle) = transport
        .listen_with_options(
            &ctx,
            ListenOptions {
                addr: "127.0.0.1:0".parse::<SocketAddr>().unwrap(),
                max_connections: None,
                tls_listener_id: Some("https".to_string()),
            },
            Arc::new(EchoAndHoldHandler {
                seen_meta: Arc::clone(&seen_meta),
                drain_notified: Arc::clone(&drain_notified),
            }),
        )
        .await
        .expect("tls listen");

    // GA03: Verify SNI routing (`a.test` verifies with cert_a, rejects cert_b; `b.test` verifies with cert_b) & ALPN.
    let raw_b_wrong = TcpStream::connect(addr).await.expect("tcp b wrong");
    let wrong = connector
        .wrap_outbound(
            &TlsClientConfig {
                server_name: "b.test".to_string(),
                alpn_protocols: vec!["http/1.1".to_string()],
                ca_cert_pem: Some(cert_a.clone()),
                insecure_skip_verify: false,
            },
            Box::new(TlsIoTcp(raw_b_wrong)),
        )
        .await;
    assert!(wrong.is_err(), "b.test must NOT validate against cert_a");

    let raw_b = TcpStream::connect(addr).await.expect("tcp b");
    let mut tls_b = connector
        .wrap_outbound(
            &TlsClientConfig {
                server_name: "b.test".to_string(),
                alpn_protocols: vec!["http/1.1".to_string()],
                ca_cert_pem: Some(cert_b.clone()),
                insecure_skip_verify: false,
            },
            Box::new(TlsIoTcp(raw_b)),
        )
        .await
        .expect("tls handshake b.test");
    assert_eq!(tls_b.alpn.as_deref(), Some(b"http/1.1".as_slice()));
    write_stream(&mut tls_b.stream, b"ping-b").await.unwrap();
    assert_eq!(read_chunk(&mut tls_b.stream).await.unwrap(), b"ping-b");
    drop(tls_b);

    // GA04 part 2: Open long-lived TLS stream on a.test (verified by cert_a).
    let raw_hold = TcpStream::connect(addr).await.expect("tcp hold");
    let mut tls_hold = connector
        .wrap_outbound(
            &TlsClientConfig {
                server_name: "a.test".to_string(),
                alpn_protocols: vec!["http/1.1".to_string()],
                ca_cert_pem: Some(cert_a.clone()),
                insecure_skip_verify: false,
            },
            Box::new(TlsIoTcp(raw_hold)),
        )
        .await
        .expect("tls handshake a.test");
    write_stream(&mut tls_hold.stream, b"HOLD").await.unwrap();
    assert_eq!(read_chunk(&mut tls_hold.stream).await.unwrap(), b"HELD");

    // Hot-reload l4-tls certificate for a.test -> cert_a2 via Loader::apply_patches!
    loader
        .apply_patches(vec![frame::EntryPatch::ReplaceConfig {
            id: "l4-tls".to_string(),
            config: tls_entry_config(
                vec![(Some("a.test"), &cert_a2, &key_a2)],
                &["http/1.1"],
            ),
            disabled: None,
        }])
        .await
        .expect("hot cert reload");
    ctx.settle().await;

    // Established TLS connection MUST still be completely alive!
    write_stream(&mut tls_hold.stream, b"still-alive")
        .await
        .expect("old TLS stream survives cert reload");
    assert_eq!(
        read_chunk(&mut tls_hold.stream).await.unwrap(),
        b"still-alive"
    );

    // New TLS connection MUST fail cert_a (old) and succeed cert_a2 (new).
    let raw_old_ca = TcpStream::connect(addr).await.expect("tcp old ca");
    let res_old_ca = connector
        .wrap_outbound(
            &TlsClientConfig {
                server_name: "a.test".to_string(),
                alpn_protocols: vec!["http/1.1".to_string()],
                ca_cert_pem: Some(cert_a),
                insecure_skip_verify: false,
            },
            Box::new(TlsIoTcp(raw_old_ca)),
        )
        .await;
    assert!(
        res_old_ca.is_err(),
        "new connection must no longer present old cert_a"
    );

    let raw_new_ca = TcpStream::connect(addr).await.expect("tcp new ca");
    let mut tls_new = connector
        .wrap_outbound(
            &TlsClientConfig {
                server_name: "a.test".to_string(),
                alpn_protocols: vec!["http/1.1".to_string()],
                ca_cert_pem: Some(cert_a2),
                insecure_skip_verify: false,
            },
            Box::new(TlsIoTcp(raw_new_ca)),
        )
        .await
        .expect("new connection validates against hot-reloaded cert_a2");
    write_stream(&mut tls_new.stream, b"new-cert-ok")
        .await
        .unwrap();
    assert_eq!(
        read_chunk(&mut tls_new.stream).await.unwrap(),
        b"new-cert-ok"
    );
    drop(tls_new);

    // GA02: Graceful drain notifies long-lived TLS stream cleanly.
    let (done_tx, done_rx) = oneshot::channel();
    tokio::spawn(async move {
        let _ = done_tx.send(listener_handle.dispose().await);
    });
    drain_notified.notified().await;
    assert_eq!(read_chunk(&mut tls_hold.stream).await.unwrap(), b"BYE");
    drop(tls_hold);
    done_rx.await.unwrap().unwrap();

    {
        let metas = seen_meta.lock().expect("metas");
        assert!(metas.iter().all(|m| m.tls));
        assert!(metas.iter().any(|m| m.alpn.as_deref() == Some("http/1.1")));
    }

    let _ = loader.close().await;
    ctx.dispose().await.expect("ctx dispose");
}
