//! Layer 2 pure-Rust RFC 6455 WebSocket engine and scoped registry plugin.

pub mod frame;
pub mod handshake;

use ::frame::{
    BoxFuture, ConfigFieldSchema, ConfigFieldType, ConfigValue, Context, Deps,
    EffectHandle, FrameError, Plugin, PluginSchema, ValidationIssue,
};
use frame::{
    FrameDecodeError, decode_frame, encode_message, parse_close_payload,
};
use handshake::{switching_protocols_headers, validate_upgrade_request};
use http_api::{
    HttpHandler, HttpRequest, HttpResponse, RouteSpec, RouterService,
    StatusCode, UpgradeHandler,
};
use net_api::{BoxStreamIo, DrainSignal};
use std::collections::BTreeMap;
use std::future::poll_fn;
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::{Mutex as AsyncMutex, mpsc};
use ws_api::{
    WsCloseFrame, WsHandler, WsMessage, WsOpcode, WsRegistry,
    WsRegistryService, WsSession, WsSessionBackend,
};

/// Resource limits and queue sizing for [`WsEnginePlugin`].
#[derive(Debug, Clone)]
pub struct WsConfig {
    pub max_frame_bytes: usize,
    pub max_message_bytes: usize,
    pub send_queue_capacity: usize,
}

impl Default for WsConfig {
    fn default() -> Self {
        Self {
            max_frame_bytes: 1024 * 1024,
            max_message_bytes: 4 * 1024 * 1024,
            send_queue_capacity: 64,
        }
    }
}

enum OutboundCmd {
    Message(WsMessage),
    Close(Option<WsCloseFrame>),
}

struct ChannelWsSession {
    out_tx: mpsc::Sender<OutboundCmd>,
    in_rx: AsyncMutex<mpsc::Receiver<io::Result<WsMessage>>>,
    path: String,
    path_params: BTreeMap<String, String>,
    remote_addr: Option<SocketAddr>,
}

impl WsSessionBackend for ChannelWsSession {
    fn send(&self, msg: WsMessage) -> BoxFuture<'static, io::Result<()>> {
        let tx = self.out_tx.clone();
        Box::pin(async move {
            tx.send(OutboundCmd::Message(msg)).await.map_err(|_| {
                io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "WebSocket connection closed",
                )
            })
        })
    }

    fn try_send(&self, msg: WsMessage) -> io::Result<()> {
        self.out_tx
            .try_send(OutboundCmd::Message(msg))
            .map_err(|e| match e {
                mpsc::error::TrySendError::Full(_) => io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "WebSocket outbound queue full",
                ),
                mpsc::error::TrySendError::Closed(_) => io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "WebSocket connection closed",
                ),
            })
    }

    fn recv(&self) -> BoxFuture<'static, Option<io::Result<WsMessage>>> {
        let rx_ptr = &self.in_rx
            as *const AsyncMutex<mpsc::Receiver<io::Result<WsMessage>>>;
        // Safety: WsSession keeps Arc<ChannelWsSession> alive for the future's lifetime?
        // No! Avoid raw pointers: lock inside Box::pin(async move) only by holding Arc<Self>!
        let _ = rx_ptr;
        unreachable!("use Arc method")
    }

    fn close(
        &self,
        frame: Option<WsCloseFrame>,
    ) -> BoxFuture<'static, io::Result<()>> {
        let tx = self.out_tx.clone();
        Box::pin(async move {
            let _ = tx.send(OutboundCmd::Close(frame)).await;
            Ok(())
        })
    }

    fn path(&self) -> &str {
        &self.path
    }

    fn path_params(&self) -> &BTreeMap<String, String> {
        &self.path_params
    }

    fn remote_addr(&self) -> Option<SocketAddr> {
        self.remote_addr
    }
}

struct ArcWsSession(Arc<ChannelWsSession>);

impl WsSessionBackend for ArcWsSession {
    fn send(&self, msg: WsMessage) -> BoxFuture<'static, io::Result<()>> {
        self.0.send(msg)
    }

    fn try_send(&self, msg: WsMessage) -> io::Result<()> {
        self.0.try_send(msg)
    }

    fn recv(&self) -> BoxFuture<'static, Option<io::Result<WsMessage>>> {
        let inner = Arc::clone(&self.0);
        Box::pin(async move {
            let mut guard = inner.in_rx.lock().await;
            guard.recv().await
        })
    }

    fn close(
        &self,
        frame: Option<WsCloseFrame>,
    ) -> BoxFuture<'static, io::Result<()>> {
        self.0.close(frame)
    }

    fn path(&self) -> &str {
        self.0.path()
    }

    fn path_params(&self) -> &BTreeMap<String, String> {
        self.0.path_params()
    }

    fn remote_addr(&self) -> Option<SocketAddr> {
        self.0.remote_addr()
    }
}

struct WsUpgradeBridge {
    config: WsConfig,
    handler: Arc<dyn WsHandler>,
    path: String,
    path_params: BTreeMap<String, String>,
    remote_addr: Option<SocketAddr>,
}

impl UpgradeHandler for WsUpgradeBridge {
    fn on_upgrade(
        &self,
        io: BoxStreamIo,
        leftover: Vec<u8>,
        drain: Arc<dyn DrainSignal>,
    ) -> BoxFuture<'static, ()> {
        let config = self.config.clone();
        let handler = Arc::clone(&self.handler);
        let path = self.path.clone();
        let path_params = self.path_params.clone();
        let remote_addr = self.remote_addr;

        Box::pin(async move {
            let (out_tx, out_rx) =
                mpsc::channel::<OutboundCmd>(config.send_queue_capacity.max(1));
            let (in_tx, in_rx) = mpsc::channel::<io::Result<WsMessage>>(64);

            let backend = Arc::new(ChannelWsSession {
                out_tx,
                in_rx: AsyncMutex::new(in_rx),
                path,
                path_params,
                remote_addr,
            });
            let session = WsSession(Arc::new(ArcWsSession(backend)));
            let app_task = tokio::spawn(async move {
                handler.on_connect(session).await;
            });

            run_ws_io_loop(io, leftover, drain, config, in_tx, out_rx).await;
            let _ = app_task.await;
        })
    }
}

async fn write_all_stream(io: &mut BoxStreamIo, data: &[u8]) -> io::Result<()> {
    let mut off = 0;
    while off < data.len() {
        let n = poll_fn(|cx| Pin::new(&mut **io).poll_write(cx, &data[off..]))
            .await?;
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "WebSocket write zero",
            ));
        }
        off += n;
    }
    poll_fn(|cx| Pin::new(&mut **io).poll_flush(cx)).await
}

struct FragmentState {
    is_text: bool,
    data: Vec<u8>,
}

async fn run_ws_io_loop(
    mut io: BoxStreamIo,
    mut buf: Vec<u8>,
    drain: Arc<dyn DrainSignal>,
    config: WsConfig,
    in_tx: mpsc::Sender<io::Result<WsMessage>>,
    mut out_rx: mpsc::Receiver<OutboundCmd>,
) {
    let mut frag: Option<FragmentState> = None;
    let mut read_eof = false;
    let drain_fut = drain.wait();
    tokio::pin!(drain_fut);

    loop {
        // First drain any already-buffered complete RFC 6455 frames before awaiting IO.
        loop {
            match decode_frame(&buf, config.max_frame_bytes) {
                Ok((f, consumed)) => {
                    buf.drain(..consumed);
                    if !f.masked {
                        let wire = encode_message(&WsMessage::Close(Some(
                            WsCloseFrame {
                                code: 1002,
                                reason: "client frames must be masked"
                                    .to_string(),
                            },
                        )));
                        let _ = write_all_stream(&mut io, &wire).await;
                        let _ = in_tx
                            .send(Err(io::Error::new(
                                io::ErrorKind::InvalidData,
                                "unmasked client frame",
                            )))
                            .await;
                        let _ =
                            poll_fn(|cx| Pin::new(&mut *io).poll_shutdown(cx))
                                .await;
                        return;
                    }

                    match f.opcode {
                        WsOpcode::Ping => {
                            let pong = encode_message(&WsMessage::Pong(
                                f.payload.clone(),
                            ));
                            if write_all_stream(&mut io, &pong).await.is_err() {
                                return;
                            }
                            let _ =
                                in_tx.try_send(Ok(WsMessage::Ping(f.payload)));
                        }
                        WsOpcode::Pong => {
                            let _ =
                                in_tx.try_send(Ok(WsMessage::Pong(f.payload)));
                        }
                        WsOpcode::Close => {
                            let close_frame =
                                match parse_close_payload(&f.payload) {
                                    Ok(cf) => cf,
                                    Err(FrameDecodeError::ProtocolError(
                                        code,
                                        reason,
                                    )) => {
                                        let wire =
                                            encode_message(&WsMessage::Close(
                                                Some(WsCloseFrame {
                                                    code,
                                                    reason: reason.to_string(),
                                                }),
                                            ));
                                        let _ =
                                            write_all_stream(&mut io, &wire)
                                                .await;
                                        return;
                                    }
                                    Err(_) => None,
                                };
                            let echo = encode_message(&WsMessage::Close(
                                close_frame.clone(),
                            ));
                            let _ = write_all_stream(&mut io, &echo).await;
                            let _ = in_tx
                                .send(Ok(WsMessage::Close(close_frame)))
                                .await;
                            let _ = poll_fn(|cx| {
                                Pin::new(&mut *io).poll_shutdown(cx)
                            })
                            .await;
                            return;
                        }
                        WsOpcode::Text | WsOpcode::Binary => {
                            if frag.is_some() {
                                let wire = encode_message(&WsMessage::Close(
                                    Some(WsCloseFrame {
                                        code: 1002,
                                        reason:
                                            "interleaved data frame inside fragment"
                                                .to_string(),
                                    }),
                                ));
                                let _ = write_all_stream(&mut io, &wire).await;
                                return;
                            }
                            let is_text = f.opcode == WsOpcode::Text;
                            if f.fin {
                                let msg = if is_text {
                                    let Ok(s) = String::from_utf8(f.payload)
                                    else {
                                        let wire =
                                            encode_message(&WsMessage::Close(
                                                Some(WsCloseFrame {
                                                    code: 1007,
                                                    reason: "invalid UTF-8"
                                                        .to_string(),
                                                }),
                                            ));
                                        let _ =
                                            write_all_stream(&mut io, &wire)
                                                .await;
                                        return;
                                    };
                                    WsMessage::Text(s)
                                } else {
                                    WsMessage::Binary(f.payload)
                                };
                                if in_tx.send(Ok(msg)).await.is_err() {
                                    return;
                                }
                            } else {
                                if f.payload.len() > config.max_message_bytes {
                                    let wire = encode_message(
                                        &WsMessage::Close(Some(WsCloseFrame {
                                            code: 1009,
                                            reason: "message too large"
                                                .to_string(),
                                        })),
                                    );
                                    let _ =
                                        write_all_stream(&mut io, &wire).await;
                                    return;
                                }
                                frag = Some(FragmentState {
                                    is_text,
                                    data: f.payload,
                                });
                            }
                        }
                        WsOpcode::Continuation => {
                            let Some(mut cur) = frag.take() else {
                                let wire = encode_message(&WsMessage::Close(
                                    Some(WsCloseFrame {
                                        code: 1002,
                                        reason: "unexpected continuation frame"
                                            .to_string(),
                                    }),
                                ));
                                let _ = write_all_stream(&mut io, &wire).await;
                                return;
                            };
                            if cur.data.len().saturating_add(f.payload.len())
                                > config.max_message_bytes
                            {
                                let wire = encode_message(&WsMessage::Close(
                                    Some(WsCloseFrame {
                                        code: 1009,
                                        reason: "message too large".to_string(),
                                    }),
                                ));
                                let _ = write_all_stream(&mut io, &wire).await;
                                return;
                            }
                            cur.data.extend_from_slice(&f.payload);
                            if f.fin {
                                let msg = if cur.is_text {
                                    let Ok(s) = String::from_utf8(cur.data)
                                    else {
                                        let wire =
                                            encode_message(&WsMessage::Close(
                                                Some(WsCloseFrame {
                                                    code: 1007,
                                                    reason: "invalid UTF-8"
                                                        .to_string(),
                                                }),
                                            ));
                                        let _ =
                                            write_all_stream(&mut io, &wire)
                                                .await;
                                        return;
                                    };
                                    WsMessage::Text(s)
                                } else {
                                    WsMessage::Binary(cur.data)
                                };
                                if in_tx.send(Ok(msg)).await.is_err() {
                                    return;
                                }
                            } else {
                                frag = Some(cur);
                            }
                        }
                    }
                }
                Err(FrameDecodeError::Incomplete) => break,
                Err(FrameDecodeError::MessageTooLarge) => {
                    let wire =
                        encode_message(&WsMessage::Close(Some(WsCloseFrame {
                            code: 1009,
                            reason: "frame too large".to_string(),
                        })));
                    let _ = write_all_stream(&mut io, &wire).await;
                    return;
                }
                Err(FrameDecodeError::ProtocolError(code, reason)) => {
                    let wire =
                        encode_message(&WsMessage::Close(Some(WsCloseFrame {
                            code,
                            reason: reason.to_string(),
                        })));
                    let _ = write_all_stream(&mut io, &wire).await;
                    return;
                }
            }
        }

        if read_eof {
            return;
        }

        let mut tmp = [0u8; 4096];
        tokio::select! {
            biased;
            _ = &mut drain_fut => {
                let wire = encode_message(&WsMessage::Close(Some(WsCloseFrame {
                    code: 1001,
                    reason: "going away".to_string(),
                })));
                let _ = write_all_stream(&mut io, &wire).await;
                let _ = poll_fn(|cx| Pin::new(&mut *io).poll_shutdown(cx)).await;
                return;
            }
            cmd = out_rx.recv() => {
                match cmd {
                    Some(OutboundCmd::Message(msg)) => {
                        let is_close = matches!(msg, WsMessage::Close(_));
                        let wire = encode_message(&msg);
                        if write_all_stream(&mut io, &wire).await.is_err() {
                            return;
                        }
                        if is_close {
                            let _ = poll_fn(|cx| Pin::new(&mut *io).poll_shutdown(cx)).await;
                            return;
                        }
                    }
                    Some(OutboundCmd::Close(cf)) => {
                        let wire = encode_message(&WsMessage::Close(cf));
                        let _ = write_all_stream(&mut io, &wire).await;
                        let _ = poll_fn(|cx| Pin::new(&mut *io).poll_shutdown(cx)).await;
                        return;
                    }
                    None => {
                        let wire = encode_message(&WsMessage::Close(Some(WsCloseFrame {
                            code: 1000,
                            reason: String::new(),
                        })));
                        let _ = write_all_stream(&mut io, &wire).await;
                        let _ = poll_fn(|cx| Pin::new(&mut *io).poll_shutdown(cx)).await;
                        return;
                    }
                }
            }
            res = poll_fn(|cx| Pin::new(&mut *io).poll_read(cx, &mut tmp)) => {
                match res {
                    Ok(0) | Err(_) => {
                        read_eof = true;
                    }
                    Ok(n) => {
                        buf.extend_from_slice(&tmp[..n]);
                    }
                }
            }
        }
    }
}

struct WsRouteHttpAdapter {
    config: WsConfig,
    handler: Arc<dyn WsHandler>,
}

impl HttpHandler for WsRouteHttpAdapter {
    fn call(&self, req: HttpRequest) -> BoxFuture<'static, HttpResponse> {
        let config = self.config.clone();
        let handler = Arc::clone(&self.handler);
        Box::pin(async move {
            let accept_key = match validate_upgrade_request(&req) {
                Ok(k) => k,
                Err(resp) => return resp,
            };
            let headers = switching_protocols_headers(&accept_key);
            let bridge: Arc<dyn http_api::UpgradeHandler> =
                Arc::new(WsUpgradeBridge {
                    config,
                    handler,
                    path: req.path,
                    path_params: req.path_params,
                    remote_addr: req.remote_addr,
                });
            HttpResponse::upgrade(
                StatusCode::SWITCHING_PROTOCOLS,
                headers,
                bridge,
            )
        })
    }
}

/// Layer 2 [`WsRegistry`] implementation mounting WebSocket endpoints onto [`RouterService`].
pub struct DefaultWsRegistry {
    router: RouterService,
    config: WsConfig,
}

impl DefaultWsRegistry {
    /// Construct a WebSocket registry backed by `router`.
    pub fn new(router: RouterService, config: WsConfig) -> Arc<Self> {
        Arc::new(Self { router, config })
    }
}

impl WsRegistry for DefaultWsRegistry {
    fn mount_ws(
        &self,
        caller: &Context,
        spec: RouteSpec,
        handler: Arc<dyn WsHandler>,
    ) -> Result<EffectHandle, FrameError> {
        let adapter: Arc<dyn HttpHandler> = Arc::new(WsRouteHttpAdapter {
            config: self.config.clone(),
            handler,
        });
        self.router.mount_route(caller, spec, adapter)
    }
}

/// Layer 2 plugin providing [`WsRegistryService`].
#[derive(Debug, Clone, Default)]
pub struct WsEnginePlugin {
    config: WsConfig,
}

impl WsEnginePlugin {
    /// Construct a WebSocket engine plugin with custom limits.
    pub fn new(config: WsConfig) -> Self {
        Self { config }
    }
}

fn parse_pos_usize(
    k: &str,
    v: &ConfigValue,
    issues: &mut Vec<ValidationIssue>,
) -> Option<usize> {
    match v {
        ConfigValue::Int(n) if *n > 0 => Some(*n as usize),
        _ => {
            issues.push(ValidationIssue {
                path: k.to_string(),
                message: format!("{k} must be a positive integer"),
            });
            None
        }
    }
}

fn resolve_ws_config(
    base: &WsConfig,
    raw: &ConfigValue,
) -> Result<WsConfig, FrameError> {
    let mut cfg = base.clone();
    match raw {
        ConfigValue::Null => Ok(cfg),
        ConfigValue::Map(map) => {
            let mut issues = Vec::new();
            for (k, v) in map {
                match k.as_str() {
                    "max_frame_bytes" => {
                        if let Some(n) = parse_pos_usize(k, v, &mut issues) {
                            cfg.max_frame_bytes = n;
                        }
                    }
                    "max_message_bytes" => {
                        if let Some(n) = parse_pos_usize(k, v, &mut issues) {
                            cfg.max_message_bytes = n;
                        }
                    }
                    "send_queue_capacity" => {
                        if let Some(n) = parse_pos_usize(k, v, &mut issues) {
                            cfg.send_queue_capacity = n;
                        }
                    }
                    _ => issues.push(ValidationIssue {
                        path: k.clone(),
                        message: format!("unknown proto-ws option `{k}`"),
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
            message: "proto-ws config must be Null or Map".to_string(),
        }])),
    }
}

impl Plugin for WsEnginePlugin {
    fn name(&self) -> &str {
        "proto-ws"
    }

    fn deps(&self) -> Deps {
        Deps::empty().with::<RouterService>()
    }

    fn schema(&self) -> PluginSchema {
        PluginSchema::new(
            "Layer 2 zero-external-WS-dependency pure-Rust RFC 6455 WebSocket engine",
        )
        .field(
            ConfigFieldSchema::new(
                "max_frame_bytes",
                ConfigFieldType::Int,
                "Maximum single WebSocket frame payload size in bytes",
            )
            .with_default(ConfigValue::Int(self.config.max_frame_bytes as i64)),
        )
        .field(
            ConfigFieldSchema::new(
                "max_message_bytes",
                ConfigFieldType::Int,
                "Maximum assembled fragmented message size in bytes",
            )
            .with_default(ConfigValue::Int(self.config.max_message_bytes as i64)),
        )
        .field(
            ConfigFieldSchema::new(
                "send_queue_capacity",
                ConfigFieldType::Int,
                "Bounded outbound queue size per WebSocket connection",
            )
            .with_default(ConfigValue::Int(
                self.config.send_queue_capacity as i64,
            )),
        )
    }

    fn validate_config(
        &self,
        raw: &ConfigValue,
    ) -> Result<ConfigValue, FrameError> {
        let _ = resolve_ws_config(&self.config, raw)?;
        Ok(raw.clone())
    }

    fn apply(
        &self,
        ctx: Context,
        config: ConfigValue,
    ) -> BoxFuture<'static, Result<(), FrameError>> {
        let base = self.config.clone();
        Box::pin(async move {
            let merged = ctx.resolve_intercept::<WsRegistryService>(config);
            let cfg = resolve_ws_config(&base, &merged)?;
            let router = ctx.get::<RouterService>().ok_or_else(|| {
                FrameError::new(
                    ::frame::ErrorKind::MissingDependency,
                    "RouterService missing for proto-ws",
                )
            })?;
            let reg = DefaultWsRegistry::new(router.as_ref().clone(), cfg);
            ctx.provide(Arc::new(WsRegistryService::new(reg))).await?;
            Ok(())
        })
    }
}
