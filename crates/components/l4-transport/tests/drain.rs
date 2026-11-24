//! BDD/TDD tests for Layer 1 L4 transport connection ledger and two-phase drain (`T-H02`).

use frame::{Context, FrameOptions};
use l4_transport::{TokioSpawner, TransportConfig, TransportPlugin};
use net_api::{
    BoxStreamIo, ConnectionHandler, ConnectionMeta, DrainSignal,
    TransportService,
};
use std::future::poll_fn;
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::{Notify, oneshot};

async fn read_one_byte(io: &mut BoxStreamIo) -> io::Result<Option<u8>> {
    let mut buf = [0u8; 1];
    let n = poll_fn(|cx| Pin::new(&mut **io).poll_read(cx, &mut buf)).await?;
    if n == 0 { Ok(None) } else { Ok(Some(buf[0])) }
}

async fn write_all_stream(io: &mut BoxStreamIo, data: &[u8]) -> io::Result<()> {
    let mut off = 0;
    while off < data.len() {
        let n = poll_fn(|cx| Pin::new(&mut **io).poll_write(cx, &data[off..]))
            .await?;
        if n == 0 {
            return Err(io::Error::new(io::ErrorKind::WriteZero, "zero write"));
        }
        off += n;
    }
    poll_fn(|cx| Pin::new(&mut **io).poll_flush(cx)).await?;
    Ok(())
}

async fn shutdown_stream(io: &mut BoxStreamIo) {
    let _ = poll_fn(|cx| Pin::new(&mut **io).poll_shutdown(cx)).await;
}

struct TwoPhaseHandler {
    events: Arc<Mutex<Vec<&'static str>>>,
    short_ready: Arc<Notify>,
    long_ready: Arc<Notify>,
}

impl ConnectionHandler for TwoPhaseHandler {
    fn handle(
        &self,
        mut io: BoxStreamIo,
        _meta: ConnectionMeta,
        drain: Arc<dyn DrainSignal>,
    ) -> frame::BoxFuture<'static, ()> {
        let events = Arc::clone(&self.events);
        let short_ready = Arc::clone(&self.short_ready);
        let long_ready = Arc::clone(&self.long_ready);
        Box::pin(async move {
            let Ok(Some(tag)) = read_one_byte(&mut io).await else {
                return;
            };
            if tag == b'S' {
                short_ready.notify_one();
                tokio::time::sleep(Duration::from_millis(50)).await;
                let _ = write_all_stream(&mut io, b"ok").await;
                shutdown_stream(&mut io).await;
                events.lock().expect("events").push("short_done");
            } else if tag == b'L' {
                drain.mark_long_lived();
                long_ready.notify_one();
                drain.wait().await;
                events.lock().expect("events").push("long_signaled");
                let _ = write_all_stream(&mut io, b"bye").await;
                shutdown_stream(&mut io).await;
                events.lock().expect("events").push("long_done");
            }
        })
    }
}

#[tokio::test]
async fn drain_stops_new_connections_waits_short_then_signals_long_lived() {
    let ctx = Context::new(FrameOptions::new(Arc::new(TokioSpawner)));
    ctx.plugin(TransportPlugin::default(), frame::ConfigValue::Null)
        .expect("transport plugin");
    ctx.settle().await;
    let transport = ctx.get::<TransportService>().expect("transport svc");

    let events = Arc::new(Mutex::new(Vec::new()));
    let short_ready = Arc::new(Notify::new());
    let long_ready = Arc::new(Notify::new());

    let handler: Arc<dyn ConnectionHandler> = Arc::new(TwoPhaseHandler {
        events: Arc::clone(&events),
        short_ready: Arc::clone(&short_ready),
        long_ready: Arc::clone(&long_ready),
    });

    let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let (bound_addr, listener_handle) =
        transport.listen(addr, handler).await.expect("listen");

    let mut conn_a = TcpStream::connect(bound_addr).await.expect("conn A");
    conn_a.write_all(b"S").await.expect("write S");
    short_ready.notified().await;

    let mut conn_b = TcpStream::connect(bound_addr).await.expect("conn B");
    conn_b.write_all(b"L").await.expect("write L");
    long_ready.notified().await;

    assert_eq!(transport.active_connections(), 2);

    let drain_started = Instant::now();
    let (done_tx, done_rx) = oneshot::channel();
    tokio::spawn(async move {
        let res = listener_handle.dispose().await;
        let _ = done_tx.send(res);
    });

    // Give the disposal task a moment to drop the OS listening socket.
    tokio::time::sleep(Duration::from_millis(5)).await;
    assert!(
        TcpStream::connect(bound_addr).await.is_err(),
        "new TCP connections must be refused immediately once drain begins"
    );

    let mut buf_a = Vec::new();
    conn_a.read_to_end(&mut buf_a).await.expect("read A");
    assert_eq!(&buf_a, b"ok");

    let mut buf_b = Vec::new();
    conn_b.read_to_end(&mut buf_b).await.expect("read B");
    assert_eq!(&buf_b, b"bye");

    done_rx
        .await
        .expect("dispose channel")
        .expect("clean dispose");
    assert!(
        drain_started.elapsed() < Duration::from_millis(500),
        "drain took too long: {:?}",
        drain_started.elapsed()
    );
    assert_eq!(transport.active_connections(), 0);
    assert_eq!(
        *events.lock().expect("events"),
        vec!["short_done", "long_signaled", "long_done"]
    );

    ctx.dispose().await.expect("ctx dispose");
}

struct StalledHandler {
    ready: Arc<Notify>,
}

impl ConnectionHandler for StalledHandler {
    fn handle(
        &self,
        _io: BoxStreamIo,
        _meta: ConnectionMeta,
        drain: Arc<dyn DrainSignal>,
    ) -> frame::BoxFuture<'static, ()> {
        let ready = Arc::clone(&self.ready);
        Box::pin(async move {
            drain.mark_long_lived();
            ready.notify_one();
            std::future::pending::<()>().await;
        })
    }
}

#[tokio::test]
async fn drain_timeout_recovers_stalled_long_lived_connection() {
    let ctx = Context::new(FrameOptions::new(Arc::new(TokioSpawner)));
    ctx.plugin(
        TransportPlugin::new(TransportConfig {
            short_drain_timeout: Duration::from_millis(50),
            long_drain_timeout: Duration::from_millis(80),
        }),
        frame::ConfigValue::Null,
    )
    .expect("transport plugin");
    ctx.settle().await;
    let transport = ctx.get::<TransportService>().expect("transport svc");

    let ready = Arc::new(Notify::new());
    let (_addr, listener_handle) = transport
        .listen(
            "127.0.0.1:0".parse().unwrap(),
            Arc::new(StalledHandler {
                ready: Arc::clone(&ready),
            }),
        )
        .await
        .expect("listen");

    let _stalled = TcpStream::connect(_addr).await.expect("connect");
    ready.notified().await;

    let started = Instant::now();
    listener_handle
        .dispose()
        .await
        .expect("dispose completes after timeout");
    assert!(
        started.elapsed() < Duration::from_millis(400),
        "stalled drain must bound exit time: {:?}",
        started.elapsed()
    );

    ctx.dispose().await.expect("ctx dispose");
}
