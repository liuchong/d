//! Layer 1 transport plugin: TCP listener management and two-phase graceful drain.

use frame::{
    BoxFuture, ConfigValue, Context, Deps, EffectHandle, ErrorKind, FrameError,
    Plugin, Spawner, ValidationIssue,
};
use net_api::{
    AsyncByteIo, BoxStreamIo, ConnectionHandler, ConnectionKind,
    ConnectionMeta, DrainSignal, Transport, TransportService,
};
use std::collections::HashMap;
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context as TaskContext, Poll};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{oneshot, watch};

/// Configuration for [`TransportPlugin`].
#[derive(Debug, Clone)]
pub struct TransportConfig {
    /// Grace window for Phase 1 waiting on in-flight short-lived connections.
    pub short_drain_timeout: Duration,
    /// Grace window for Phase 2 waiting after notifying long-lived connections.
    pub long_drain_timeout: Duration,
}

impl Default for TransportConfig {
    fn default() -> Self {
        Self {
            short_drain_timeout: Duration::from_secs(5),
            long_drain_timeout: Duration::from_secs(5),
        }
    }
}

/// [`frame::Spawner`] backed by the active Tokio runtime handle.
#[derive(Debug, Default)]
pub struct TokioSpawner;

impl Spawner for TokioSpawner {
    fn spawn(&self, fut: BoxFuture<'static, ()>) {
        tokio::spawn(fut);
    }
}

struct TcpStreamIo(TcpStream);

impl AsyncByteIo for TcpStreamIo {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        let mut read_buf = ReadBuf::new(buf);
        match Pin::new(&mut self.0).poll_read(cx, &mut read_buf) {
            Poll::Ready(Ok(())) => Poll::Ready(Ok(read_buf.filled().len())),
            Poll::Ready(Err(e)) => Poll::Ready(Err(e)),
            Poll::Pending => Poll::Pending,
        }
    }

    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.0).poll_write(cx, buf)
    }

    fn poll_flush(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_shutdown(cx)
    }
}

struct Ledger {
    conns: Mutex<HashMap<u64, ConnectionKind>>,
    counts_tx: watch::Sender<(usize, usize)>,
    _counts_rx: watch::Receiver<(usize, usize)>,
    global_active: Arc<AtomicUsize>,
    draining: AtomicBool,
    phase2_tx: watch::Sender<bool>,
    _phase2_rx: watch::Receiver<bool>,
}

impl Ledger {
    fn new(global_active: Arc<AtomicUsize>) -> Arc<Self> {
        let (counts_tx, counts_rx) = watch::channel((0, 0));
        let (phase2_tx, phase2_rx) = watch::channel(false);
        Arc::new(Self {
            conns: Mutex::new(HashMap::new()),
            counts_tx,
            _counts_rx: counts_rx,
            global_active,
            draining: AtomicBool::new(false),
            phase2_tx,
            _phase2_rx: phase2_rx,
        })
    }

    fn recompute_locked(map: &HashMap<u64, ConnectionKind>) -> (usize, usize) {
        let short = map
            .values()
            .filter(|k| matches!(k, ConnectionKind::ShortLived))
            .count();
        (short, map.len())
    }

    fn insert(&self, id: u64, kind: ConnectionKind) {
        let mut map = self.conns.lock().unwrap_or_else(|e| e.into_inner());
        map.insert(id, kind);
        let counts = Self::recompute_locked(&map);
        self.global_active.fetch_add(1, Ordering::AcqRel);
        let _ = self.counts_tx.send(counts);
    }

    fn set_kind(&self, id: u64, kind: ConnectionKind) {
        let mut map = self.conns.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(slot) = map.get_mut(&id)
            && *slot != kind
        {
            *slot = kind;
            let counts = Self::recompute_locked(&map);
            let _ = self.counts_tx.send(counts);
        }
    }

    fn remove(&self, id: u64) {
        let mut map = self.conns.lock().unwrap_or_else(|e| e.into_inner());
        if map.remove(&id).is_some() {
            let counts = Self::recompute_locked(&map);
            self.global_active.fetch_sub(1, Ordering::AcqRel);
            let _ = self.counts_tx.send(counts);
        }
    }

    async fn wait_short_drained(&self, timeout: Duration) {
        let mut rx = self.counts_tx.subscribe();
        let _ = tokio::time::timeout(
            timeout,
            rx.wait_for(|&(short, _)| short == 0),
        )
        .await;
    }

    async fn wait_all_drained(&self, timeout: Duration) {
        let _ = self.phase2_tx.send(true);
        let mut rx = self.counts_tx.subscribe();
        let _ = tokio::time::timeout(
            timeout,
            rx.wait_for(|&(_, total)| total == 0),
        )
        .await;
    }
}

struct ConnGuard {
    id: u64,
    ledger: Arc<Ledger>,
}

impl Drop for ConnGuard {
    fn drop(&mut self) {
        self.ledger.remove(self.id);
    }
}

struct ConnectionDrainSignal {
    id: u64,
    ledger: Arc<Ledger>,
}

impl DrainSignal for ConnectionDrainSignal {
    fn wait(&self) -> BoxFuture<'static, ()> {
        let mut rx = self.ledger.phase2_tx.subscribe();
        Box::pin(async move {
            let _ = rx.wait_for(|&signaled| signaled).await;
        })
    }

    fn is_draining(&self) -> bool {
        self.ledger.draining.load(Ordering::Acquire)
    }

    fn set_kind(&self, kind: ConnectionKind) {
        self.ledger.set_kind(self.id, kind);
    }
}

/// Layer 1 [`Transport`] service implementation.
pub struct DefaultTransportService {
    config: TransportConfig,
    next_conn_id: Arc<AtomicU64>,
    global_active: Arc<AtomicUsize>,
}

impl DefaultTransportService {
    /// Construct a standalone transport service.
    pub fn new(config: TransportConfig) -> Arc<Self> {
        Arc::new(Self {
            config,
            next_conn_id: Arc::new(AtomicU64::new(1)),
            global_active: Arc::new(AtomicUsize::new(0)),
        })
    }
}

impl Transport for DefaultTransportService {
    fn listen(
        &self,
        caller: &Context,
        addr: SocketAddr,
        handler: Arc<dyn ConnectionHandler>,
    ) -> BoxFuture<'static, Result<(SocketAddr, EffectHandle), FrameError>>
    {
        let ctx = caller.clone();
        let config = self.config.clone();
        let next_conn_id = Arc::clone(&self.next_conn_id);
        let global_active = Arc::clone(&self.global_active);

        Box::pin(async move {
            let listener = TcpListener::bind(addr).await.map_err(|e| {
                FrameError::new(
                    ErrorKind::ActivationFailed,
                    format!("failed to bind {addr}: {e}"),
                )
            })?;
            let local_addr = listener.local_addr().map_err(|e| {
                FrameError::new(
                    ErrorKind::ActivationFailed,
                    format!("failed to inspect local address for {addr}: {e}"),
                )
            })?;

            let ledger = Ledger::new(global_active);
            let (stop_tx, mut stop_rx) = watch::channel(false);
            let (stopped_tx, stopped_rx) = oneshot::channel::<()>();

            let accept_ledger = Arc::clone(&ledger);
            tokio::spawn(async move {
                loop {
                    tokio::select! {
                        biased;
                        res = stop_rx.wait_for(|&stopped| stopped) => {
                            let _ = res;
                            break;
                        }
                        accepted = listener.accept() => {
                            match accepted {
                                Ok((stream, remote_addr)) => {
                                    let _ = stream.set_nodelay(true);
                                    let id = next_conn_id.fetch_add(1, Ordering::Relaxed);
                                    let meta = ConnectionMeta {
                                        id,
                                        remote_addr,
                                        kind: ConnectionKind::ShortLived,
                                    };
                                    accept_ledger.insert(id, ConnectionKind::ShortLived);
                                    let guard = ConnGuard {
                                        id,
                                        ledger: Arc::clone(&accept_ledger),
                                    };
                                    let drain: Arc<dyn DrainSignal> = Arc::new(ConnectionDrainSignal {
                                        id,
                                        ledger: Arc::clone(&accept_ledger),
                                    });
                                    let h = Arc::clone(&handler);
                                    let io: BoxStreamIo = Box::new(TcpStreamIo(stream));
                                    tokio::spawn(async move {
                                        let _guard = guard;
                                        h.handle(io, meta, drain).await;
                                    });
                                }
                                Err(_) => break,
                            }
                        }
                    }
                }
                // Drop the OS listener immediately before acknowledging stop so
                // subsequent connects fail right away instead of hitting backlog.
                drop(listener);
                let _ = stopped_tx.send(());
            });

            let effect = ctx
                .effect(format!("l4-listener:{local_addr}"), move |scope| {
                    Box::pin(async move {
                        scope.defer_async(move || {
                            Box::pin(async move {
                                ledger.draining.store(true, Ordering::Release);
                                let _ = stop_tx.send(true);
                                let _ = stopped_rx.await;
                                ledger
                                    .wait_short_drained(
                                        config.short_drain_timeout,
                                    )
                                    .await;
                                ledger
                                    .wait_all_drained(config.long_drain_timeout)
                                    .await;
                                Ok(())
                            })
                        });
                        Ok(())
                    })
                })
                .await?;

            Ok((local_addr, effect))
        })
    }

    fn active_connections(&self) -> usize {
        self.global_active.load(Ordering::Acquire)
    }
}

/// Layer 1 plugin providing [`TransportService`].
#[derive(Debug, Clone, Default)]
pub struct TransportPlugin {
    config: TransportConfig,
}

impl TransportPlugin {
    /// Create a transport plugin with custom drain timeouts.
    pub fn new(config: TransportConfig) -> Self {
        Self { config }
    }
}

fn resolve_transport_config(
    base: &TransportConfig,
    raw: &ConfigValue,
) -> Result<TransportConfig, FrameError> {
    let mut cfg = base.clone();
    match raw {
        ConfigValue::Null => Ok(cfg),
        ConfigValue::Map(map) => {
            let mut issues = Vec::new();
            for (k, v) in map {
                match k.as_str() {
                    "short_drain_ms" => match v {
                        ConfigValue::Int(ms) if *ms >= 0 => {
                            cfg.short_drain_timeout =
                                Duration::from_millis(*ms as u64);
                        }
                        _ => issues.push(ValidationIssue {
                            path: k.clone(),
                            message:
                                "short_drain_ms must be a non-negative integer"
                                    .to_string(),
                        }),
                    },
                    "long_drain_ms" => match v {
                        ConfigValue::Int(ms) if *ms >= 0 => {
                            cfg.long_drain_timeout =
                                Duration::from_millis(*ms as u64);
                        }
                        _ => issues.push(ValidationIssue {
                            path: k.clone(),
                            message:
                                "long_drain_ms must be a non-negative integer"
                                    .to_string(),
                        }),
                    },
                    _ => issues.push(ValidationIssue {
                        path: k.clone(),
                        message: format!("unknown l4-transport option `{k}`"),
                    }),
                }
            }
            if issues.is_empty() {
                Ok(cfg)
            } else {
                Err(FrameError::validation(issues))
            }
        }
        _ => Err(FrameError::validation(vec![ValidationIssue {
            path: "".to_string(),
            message: "l4-transport config must be Null or Map".to_string(),
        }])),
    }
}

impl Plugin for TransportPlugin {
    fn name(&self) -> &str {
        "l4-transport"
    }

    fn deps(&self) -> Deps {
        Deps::empty()
    }

    fn validate_config(
        &self,
        raw: &ConfigValue,
    ) -> Result<ConfigValue, FrameError> {
        let _ = resolve_transport_config(&self.config, raw)?;
        Ok(raw.clone())
    }

    fn apply(
        &self,
        ctx: Context,
        config: ConfigValue,
    ) -> BoxFuture<'static, Result<(), FrameError>> {
        let base = self.config.clone();
        Box::pin(async move {
            let merged = ctx.resolve_intercept::<TransportService>(config);
            let cfg = resolve_transport_config(&base, &merged)?;
            let svc = DefaultTransportService::new(cfg);
            let _handle =
                ctx.provide(Arc::new(TransportService::new(svc))).await?;
            Ok(())
        })
    }
}
