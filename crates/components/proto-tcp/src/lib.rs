//! Layer 2 raw TCP protocol engine and declarative L4 stream forwarder.

use frame::{
    BoxFuture, ConfigFieldSchema, ConfigFieldType, ConfigValue, Context, Deps,
    EffectHandle, FrameError, Plugin, PluginSchema, ScopedService, Service,
    ValidationIssue,
};
use net_api::{
    BoxStreamIo, ConnectionHandler, ConnectionMeta, DrainSignal, ListenOptions,
    TransportService,
};
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context as TaskContext, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;

/// Declarative L4 TCP port forwarding rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TcpForwardRule {
    pub id: String,
    pub listen_addr: SocketAddr,
    pub target_addr: SocketAddr,
    pub max_connections: Option<usize>,
    pub tls_listener_id: Option<String>,
}

/// Configuration for [`TcpEnginePlugin`].
#[derive(Debug, Clone, Default)]
pub struct TcpConfig {
    pub rules: Vec<TcpForwardRule>,
}

/// Contract for mounting raw Layer 2 TCP protocol handlers atop [`TransportService`].
pub trait TcpRegistry: Send + Sync + 'static {
    /// Mount a raw TCP [`ConnectionHandler`] owned by `caller` using `opts`.
    fn mount_tcp(
        &self,
        caller: &Context,
        opts: ListenOptions,
        handler: Arc<dyn ConnectionHandler>,
    ) -> BoxFuture<'static, Result<(SocketAddr, EffectHandle), FrameError>>;
}

/// Frame service handle exposing raw TCP mounting to custom Layer 4 plugins.
#[derive(Clone)]
pub struct TcpRegistryService(pub Arc<dyn TcpRegistry>);

impl TcpRegistryService {
    /// Wrap a [`TcpRegistry`] implementation as a service handle.
    pub fn new(inner: Arc<dyn TcpRegistry>) -> Self {
        Self(inner)
    }

    /// Delegate [`TcpRegistry::mount_tcp`] with explicit caller ownership.
    pub fn mount_tcp(
        &self,
        caller: &Context,
        opts: ListenOptions,
        handler: Arc<dyn ConnectionHandler>,
    ) -> BoxFuture<'static, Result<(SocketAddr, EffectHandle), FrameError>>
    {
        self.0.mount_tcp(caller, opts, handler)
    }
}

/// Caller-bound [`TcpRegistryService`] view obtained via `ctx.use_service::<TcpRegistryService>()`.
#[derive(Clone)]
pub struct ScopedTcpRegistry {
    inner: Arc<TcpRegistryService>,
    caller: Context,
}

impl ScopedTcpRegistry {
    /// Bind a raw TCP listener owned by the caller fiber.
    pub fn mount_tcp(
        &self,
        opts: ListenOptions,
        handler: Arc<dyn ConnectionHandler>,
    ) -> BoxFuture<'static, Result<(SocketAddr, EffectHandle), FrameError>>
    {
        self.inner.mount_tcp(&self.caller, opts, handler)
    }
}

impl Service for TcpRegistryService {
    const NAME: &'static str = "tcp.registry";
}

impl ScopedService for TcpRegistryService {
    type View = ScopedTcpRegistry;

    fn bind(self: Arc<Self>, caller: Context) -> Self::View {
        ScopedTcpRegistry {
            inner: self,
            caller,
        }
    }
}

struct DefaultTcpRegistry {
    transport: TransportService,
}

impl TcpRegistry for DefaultTcpRegistry {
    fn mount_tcp(
        &self,
        caller: &Context,
        opts: ListenOptions,
        handler: Arc<dyn ConnectionHandler>,
    ) -> BoxFuture<'static, Result<(SocketAddr, EffectHandle), FrameError>>
    {
        self.transport.listen_with_options(caller, opts, handler)
    }
}

struct TokioCompat(BoxStreamIo);

impl AsyncRead for TokioCompat {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let slice = buf.initialize_unfilled();
        match Pin::new(&mut *self.0).poll_read(cx, slice) {
            Poll::Ready(Ok(n)) => {
                buf.advance(n);
                Poll::Ready(Ok(()))
            }
            Poll::Ready(Err(e)) => Poll::Ready(Err(e)),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl AsyncWrite for TokioCompat {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut *self.0).poll_write(cx, buf)
    }

    fn poll_flush(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut *self.0).poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut *self.0).poll_shutdown(cx)
    }
}

struct TcpForwardHandler {
    target_addr: SocketAddr,
}

impl ConnectionHandler for TcpForwardHandler {
    fn handle(
        &self,
        io: BoxStreamIo,
        _meta: ConnectionMeta,
        drain: Arc<dyn DrainSignal>,
    ) -> BoxFuture<'static, ()> {
        let target_addr = self.target_addr;
        Box::pin(async move {
            drain.mark_long_lived();
            let Ok(mut upstream) = TcpStream::connect(target_addr).await else {
                return;
            };
            let _ = upstream.set_nodelay(true);
            let mut downstream = TokioCompat(io);
            let drain_fut = drain.wait();
            tokio::pin!(drain_fut);
            tokio::select! {
                biased;
                _ = &mut drain_fut => {}
                _ = tokio::io::copy_bidirectional(&mut downstream, &mut upstream) => {}
            }
        })
    }
}

/// Layer 2 plugin providing [`TcpRegistryService`] and optional declarative TCP forwarders.
#[derive(Debug, Clone, Default)]
pub struct TcpEnginePlugin {
    config: TcpConfig,
}

impl TcpEnginePlugin {
    /// Construct a TCP engine plugin with static forwarding rules.
    pub fn new(config: TcpConfig) -> Self {
        Self { config }
    }
}

fn resolve_tcp_config(
    base: &TcpConfig,
    raw: &ConfigValue,
) -> Result<TcpConfig, FrameError> {
    let mut cfg = base.clone();
    match raw {
        ConfigValue::Null => Ok(cfg),
        ConfigValue::Map(map) => {
            let mut issues = Vec::new();
            for (k, v) in map {
                match k.as_str() {
                    "rules" => {
                        let ConfigValue::List(arr) = v else {
                            issues.push(ValidationIssue {
                                path: "rules".to_string(),
                                message: "rules must be a list".to_string(),
                            });
                            continue;
                        };
                        cfg.rules.clear();
                        for (idx, item) in arr.iter().enumerate() {
                            let prefix = format!("rules[{idx}]");
                            let ConfigValue::Map(rmap) = item else {
                                issues.push(ValidationIssue {
                                    path: prefix,
                                    message: "rule entry must be a map"
                                        .to_string(),
                                });
                                continue;
                            };
                            let mut id = None;
                            let mut listen_addr = None;
                            let mut target_addr = None;
                            let mut max_connections = None;
                            let mut tls_listener_id = None;
                            for (rk, rv) in rmap {
                                match rk.as_str() {
                                    "id" => match rv {
                                        ConfigValue::Str(s)
                                            if !s.trim().is_empty() =>
                                        {
                                            id = Some(s.clone());
                                        }
                                        _ => issues.push(ValidationIssue {
                                            path: format!("{prefix}.id"),
                                            message: "rule id must be non-empty string".to_string(),
                                        }),
                                    },
                                    "listen_addr" => match rv {
                                        ConfigValue::Str(s) => match s.parse::<SocketAddr>() {
                                            Ok(a) => listen_addr = Some(a),
                                            Err(e) => issues.push(ValidationIssue {
                                                path: format!("{prefix}.listen_addr"),
                                                message: format!("invalid listen_addr `{s}`: {e}"),
                                            }),
                                        },
                                        _ => issues.push(ValidationIssue {
                                            path: format!("{prefix}.listen_addr"),
                                            message: "listen_addr must be a socket address string".to_string(),
                                        }),
                                    },
                                    "target_addr" => match rv {
                                        ConfigValue::Str(s) => match s.parse::<SocketAddr>() {
                                            Ok(a) => target_addr = Some(a),
                                            Err(e) => issues.push(ValidationIssue {
                                                path: format!("{prefix}.target_addr"),
                                                message: format!("invalid target_addr `{s}`: {e}"),
                                            }),
                                        },
                                        _ => issues.push(ValidationIssue {
                                            path: format!("{prefix}.target_addr"),
                                            message: "target_addr must be a socket address string".to_string(),
                                        }),
                                    },
                                    "max_connections" => match rv {
                                        ConfigValue::Null => max_connections = None,
                                        ConfigValue::Int(n) if *n > 0 => {
                                            max_connections = Some(*n as usize);
                                        }
                                        _ => issues.push(ValidationIssue {
                                            path: format!("{prefix}.max_connections"),
                                            message: "max_connections must be a positive integer or null".to_string(),
                                        }),
                                    },
                                    "tls_listener_id" => match rv {
                                        ConfigValue::Null => tls_listener_id = None,
                                        ConfigValue::Str(s) if !s.trim().is_empty() => {
                                            tls_listener_id = Some(s.clone());
                                        }
                                        _ => issues.push(ValidationIssue {
                                            path: format!("{prefix}.tls_listener_id"),
                                            message: "tls_listener_id must be a non-empty string or null".to_string(),
                                        }),
                                    },
                                    _ => issues.push(ValidationIssue {
                                        path: format!("{prefix}.{rk}"),
                                        message: format!("unknown proto-tcp rule field `{rk}`"),
                                    }),
                                }
                            }
                            if let (
                                Some(id),
                                Some(listen_addr),
                                Some(target_addr),
                            ) = (id, listen_addr, target_addr)
                            {
                                cfg.rules.push(TcpForwardRule {
                                    id,
                                    listen_addr,
                                    target_addr,
                                    max_connections,
                                    tls_listener_id,
                                });
                            } else {
                                issues.push(ValidationIssue {
                                    path: prefix,
                                    message: "rule requires id, listen_addr, and target_addr".to_string(),
                                });
                            }
                        }
                    }
                    _ => issues.push(ValidationIssue {
                        path: k.clone(),
                        message: format!("unknown proto-tcp option `{k}`"),
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
            message: "proto-tcp config must be Null or Map".to_string(),
        }])),
    }
}

impl Plugin for TcpEnginePlugin {
    fn name(&self) -> &str {
        "proto-tcp"
    }

    fn deps(&self) -> Deps {
        Deps::empty().with::<TransportService>()
    }

    fn schema(&self) -> PluginSchema {
        PluginSchema::new(
            "Layer 2 raw TCP handler registry and declarative L4 stream forwarder",
        )
        .field(ConfigFieldSchema::new(
            "rules",
            ConfigFieldType::List,
            "Declarative L4 forwarding rules ([{id, listen_addr, target_addr, max_connections, tls_listener_id}])",
        ))
    }

    fn validate_config(
        &self,
        raw: &ConfigValue,
    ) -> Result<ConfigValue, FrameError> {
        let _ = resolve_tcp_config(&self.config, raw)?;
        Ok(raw.clone())
    }

    fn apply(
        &self,
        ctx: Context,
        config: ConfigValue,
    ) -> BoxFuture<'static, Result<(), FrameError>> {
        let base = self.config.clone();
        Box::pin(async move {
            let merged = ctx.resolve_intercept::<TcpRegistryService>(config);
            let cfg = resolve_tcp_config(&base, &merged)?;
            let transport = ctx.use_service::<TransportService>()?;

            let transport_raw =
                ctx.get::<TransportService>().ok_or_else(|| {
                    FrameError::new(
                        frame::ErrorKind::MissingDependency,
                        "TransportService missing",
                    )
                })?;
            let reg: Arc<dyn TcpRegistry> = Arc::new(DefaultTcpRegistry {
                transport: transport_raw.as_ref().clone(),
            });
            ctx.provide(Arc::new(TcpRegistryService::new(reg))).await?;

            for rule in cfg.rules {
                let handler: Arc<dyn ConnectionHandler> =
                    Arc::new(TcpForwardHandler {
                        target_addr: rule.target_addr,
                    });
                let _ = transport
                    .listen_with_options(
                        ListenOptions {
                            addr: rule.listen_addr,
                            max_connections: rule.max_connections,
                            tls_listener_id: rule.tls_listener_id,
                        },
                        handler,
                    )
                    .await?;
            }

            Ok(())
        })
    }
}
