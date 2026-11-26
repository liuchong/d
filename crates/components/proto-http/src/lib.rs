//! Layer 2 self-built HTTP/1.1 engine and lock-free snapshot router.

pub mod parser;
pub mod router;
pub mod writer;

use frame::{
    BoxFuture, ConfigFieldSchema, ConfigFieldType, ConfigValue, Context, Deps,
    ErrorKind, FrameError, Plugin, PluginSchema, ValidationIssue,
};
pub use http_api::BoundServerInfo;
use http_api::{
    HttpHandler, HttpRequest, HttpResponse, RouterService, Version,
};
use net_api::{
    BoxStreamIo, ConnectionHandler, ConnectionMeta, DrainSignal, ListenOptions,
    TransportService,
};
use parser::ConnReader;
pub use router::HttpRouter;
use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use writer::{WriteOutcome, write_response};

/// Additional listener binding configuration for [`HttpEnginePlugin`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpListenerConfig {
    pub bind_addr: SocketAddr,
    pub tls_listener_id: Option<String>,
    pub max_connections: Option<usize>,
}

/// HTTP/1.1 parser, listener, and connection settings.
#[derive(Debug, Clone)]
pub struct HttpConfig {
    pub bind_addr: Option<SocketAddr>,
    pub tls_listener_id: Option<String>,
    pub max_connections: Option<usize>,
    pub extra_listeners: Vec<HttpListenerConfig>,
    pub max_request_line_bytes: usize,
    pub max_headers: usize,
    pub max_header_bytes: usize,
    pub max_body_bytes: usize,
    pub stream_body_threshold: usize,
    pub idle_timeout: Duration,
}

impl Default for HttpConfig {
    fn default() -> Self {
        Self {
            bind_addr: None,
            tls_listener_id: None,
            max_connections: None,
            extra_listeners: Vec::new(),
            max_request_line_bytes: 8 * 1024,
            max_headers: 128,
            max_header_bytes: 64 * 1024,
            max_body_bytes: 64 * 1024 * 1024,
            stream_body_threshold: 64 * 1024,
            idle_timeout: Duration::from_secs(30),
        }
    }
}

struct FnHandler<F>(F);

impl<F, Fut> HttpHandler for FnHandler<F>
where
    F: Fn(HttpRequest) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = HttpResponse> + Send + 'static,
{
    fn call(&self, req: HttpRequest) -> BoxFuture<'static, HttpResponse> {
        Box::pin((self.0)(req))
    }
}

/// Wrap an async closure as a boxed [`HttpHandler`].
pub fn fn_handler<F, Fut>(f: F) -> Arc<dyn HttpHandler>
where
    F: Fn(HttpRequest) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = HttpResponse> + Send + 'static,
{
    Arc::new(FnHandler(f))
}

struct HttpConnectionDriver {
    router: Arc<HttpRouter>,
    config: HttpConfig,
}

impl ConnectionHandler for HttpConnectionDriver {
    fn handle(
        &self,
        io: BoxStreamIo,
        meta: ConnectionMeta,
        drain: Arc<dyn DrainSignal>,
    ) -> BoxFuture<'static, ()> {
        let router = Arc::clone(&self.router);
        let config = self.config.clone();
        Box::pin(async move {
            let mut reader = ConnReader::new(io);
            loop {
                match reader
                    .read_head(&config, meta.remote_addr, meta.tls, &drain)
                    .await
                {
                    Ok(Some(head)) => {
                        let version = head.request.version;
                        let is_head =
                            head.request.method == http_api::Method::Head;
                        let mut keep_alive = head.keep_alive;
                        let resp = if matches!(
                            head.pump,
                            parser::BodyPump::None
                        ) {
                            <HttpRouter as http_api::Router>::dispatch(
                                &router,
                                head.request,
                            )
                            .await
                        } else {
                            let dispatch_fut =
                                <HttpRouter as http_api::Router>::dispatch(
                                    &router,
                                    head.request,
                                );
                            let (resp_opt, pump_err) = {
                                let pump_fut = reader.pump_body(
                                    head.pump,
                                    config.max_body_bytes,
                                    config.idle_timeout,
                                );
                                tokio::pin!(dispatch_fut);
                                tokio::pin!(pump_fut);
                                let mut resp_opt: Option<HttpResponse> = None;
                                let mut pump_err: Option<
                                    parser::ParseRejection,
                                > = None;
                                let mut pump_done = false;
                                loop {
                                    tokio::select! {
                                        biased;
                                        p_res = &mut pump_fut, if !pump_done => {
                                            pump_done = true;
                                            if let Err(rej) = p_res {
                                                pump_err = Some(rej);
                                                break;
                                            }
                                        }
                                        r = &mut dispatch_fut, if resp_opt.is_none() => {
                                            resp_opt = Some(r);
                                            if pump_done {
                                                break;
                                            }
                                            keep_alive = false;
                                            break;
                                        }
                                    }
                                    if resp_opt.is_some() && pump_done {
                                        break;
                                    }
                                }
                                (resp_opt, pump_err)
                            };
                            if let Some(rej) = pump_err {
                                if let Some((status, msg)) =
                                    rej.status_and_message()
                                {
                                    let err_resp =
                                        HttpResponse::text(status, msg);
                                    let _ = write_response(
                                        reader.io_mut(),
                                        Version::Http11,
                                        false,
                                        false,
                                        err_resp,
                                        &drain,
                                    )
                                    .await;
                                }
                                break;
                            }
                            resp_opt.expect("dispatch must produce response")
                        };

                        match write_response(
                            reader.io_mut(),
                            version,
                            is_head,
                            keep_alive,
                            resp,
                            &drain,
                        )
                        .await
                        {
                            Ok(WriteOutcome::KeepAlive) => continue,
                            Ok(WriteOutcome::Close) | Err(_) => break,
                            Ok(WriteOutcome::Upgraded(upgrade_handler)) => {
                                let (io, leftover) =
                                    reader.take_io_and_leftover();
                                upgrade_handler
                                    .on_upgrade(io, leftover, drain)
                                    .await;
                                return;
                            }
                        }
                    }
                    Ok(None) => break,
                    Err(rej) => {
                        if let Some((status, msg)) = rej.status_and_message() {
                            let resp = HttpResponse::text(status, msg);
                            let _ = write_response(
                                reader.io_mut(),
                                Version::Http11,
                                false,
                                false,
                                resp,
                                &drain,
                            )
                            .await;
                        }
                        let mut io = reader.into_io();
                        let _ = std::future::poll_fn(|cx| {
                            std::pin::Pin::new(&mut *io).poll_shutdown(cx)
                        })
                        .await;
                        // Discard unread pipelined/oversized client bytes briefly after FIN
                        // so the kernel does not RST before the peer reads the 4xx/5xx reply.
                        let _ = tokio::time::timeout(
                            Duration::from_millis(50),
                            drain_unread_tail(&mut io, 512 * 1024),
                        )
                        .await;
                        return;
                    }
                }
            }
            let mut io = reader.into_io();
            let _ = std::future::poll_fn(|cx| {
                std::pin::Pin::new(&mut *io).poll_shutdown(cx)
            })
            .await;
        })
    }
}

async fn drain_unread_tail(io: &mut BoxStreamIo, max_discard: usize) {
    let mut buf = [0u8; 4096];
    let mut discarded = 0usize;
    while discarded < max_discard {
        match std::future::poll_fn(|cx| {
            std::pin::Pin::new(&mut **io).poll_read(cx, &mut buf)
        })
        .await
        {
            Ok(0) | Err(_) => break,
            Ok(n) => discarded += n,
        }
    }
}

/// Layer 2 plugin providing [`RouterService`] and optional L4 HTTP listener binding.
#[derive(Debug, Clone, Default)]
pub struct HttpEnginePlugin {
    config: HttpConfig,
}

impl HttpEnginePlugin {
    /// Create an HTTP engine plugin with `config`.
    pub fn new(config: HttpConfig) -> Self {
        Self { config }
    }
}

fn parse_positive_usize(
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

fn resolve_http_config(
    base: &HttpConfig,
    raw: &ConfigValue,
) -> Result<HttpConfig, FrameError> {
    let mut cfg = base.clone();
    match raw {
        ConfigValue::Null => Ok(cfg),
        ConfigValue::Map(map) => {
            let mut issues = Vec::new();
            for (k, v) in map {
                match k.as_str() {
                    "bind_addr" => match v {
                        ConfigValue::Null => cfg.bind_addr = None,
                        ConfigValue::Str(s) => match s.parse::<SocketAddr>() {
                            Ok(addr) => cfg.bind_addr = Some(addr),
                            Err(e) => issues.push(ValidationIssue {
                                path: k.clone(),
                                message: format!("invalid bind_addr `{s}`: {e}"),
                            }),
                        },
                        _ => issues.push(ValidationIssue {
                            path: k.clone(),
                            message: "bind_addr must be a socket address string or null"
                                .to_string(),
                        }),
                    },
                    "tls_listener_id" => match v {
                        ConfigValue::Null => cfg.tls_listener_id = None,
                        ConfigValue::Str(s) if !s.trim().is_empty() => {
                            cfg.tls_listener_id = Some(s.clone());
                        }
                        _ => issues.push(ValidationIssue {
                            path: k.clone(),
                            message: "tls_listener_id must be a non-empty string or null"
                                .to_string(),
                        }),
                    },
                    "max_connections" => match v {
                        ConfigValue::Null => cfg.max_connections = None,
                        ConfigValue::Int(n) if *n > 0 => {
                            cfg.max_connections = Some(*n as usize);
                        }
                        _ => issues.push(ValidationIssue {
                            path: k.clone(),
                            message: "max_connections must be a positive integer or null"
                                .to_string(),
                        }),
                    },
                    "extra_listeners" => {
                        let ConfigValue::List(arr) = v else {
                            issues.push(ValidationIssue {
                                path: k.clone(),
                                message: "extra_listeners must be a list"
                                    .to_string(),
                            });
                            continue;
                        };
                        cfg.extra_listeners.clear();
                        for (idx, item) in arr.iter().enumerate() {
                            let prefix = format!("extra_listeners[{idx}]");
                            let ConfigValue::Map(lmap) = item else {
                                issues.push(ValidationIssue {
                                    path: prefix,
                                    message: "listener entry must be a map"
                                        .to_string(),
                                });
                                continue;
                            };
                            let mut bind_addr = None;
                            let mut tls_id = None;
                            let mut max_conns = None;
                            for (lk, lv) in lmap {
                                match lk.as_str() {
                                    "bind_addr" => match lv {
                                        ConfigValue::Str(s) => {
                                            match s.parse::<SocketAddr>() {
                                                Ok(a) => bind_addr = Some(a),
                                                Err(e) => issues.push(
                                                    ValidationIssue {
                                                        path: format!("{prefix}.bind_addr"),
                                                        message: format!("invalid bind_addr `{s}`: {e}"),
                                                    },
                                                ),
                                            }
                                        }
                                        _ => issues.push(ValidationIssue {
                                            path: format!("{prefix}.bind_addr"),
                                            message: "bind_addr must be string"
                                                .to_string(),
                                        }),
                                    },
                                    "tls_listener_id" => match lv {
                                        ConfigValue::Null => tls_id = None,
                                        ConfigValue::Str(s)
                                            if !s.trim().is_empty() =>
                                        {
                                            tls_id = Some(s.clone());
                                        }
                                        _ => issues.push(ValidationIssue {
                                            path: format!(
                                                "{prefix}.tls_listener_id"
                                            ),
                                            message: "tls_listener_id must be string or null"
                                                .to_string(),
                                        }),
                                    },
                                    "max_connections" => match lv {
                                        ConfigValue::Null => max_conns = None,
                                        ConfigValue::Int(n) if *n > 0 => {
                                            max_conns = Some(*n as usize);
                                        }
                                        _ => issues.push(ValidationIssue {
                                            path: format!(
                                                "{prefix}.max_connections"
                                            ),
                                            message: "max_connections must be positive integer or null"
                                                .to_string(),
                                        }),
                                    },
                                    _ => issues.push(ValidationIssue {
                                        path: format!("{prefix}.{lk}"),
                                        message: format!(
                                            "unknown listener option `{lk}`"
                                        ),
                                    }),
                                }
                            }
                            if let Some(addr) = bind_addr {
                                cfg.extra_listeners.push(HttpListenerConfig {
                                    bind_addr: addr,
                                    tls_listener_id: tls_id,
                                    max_connections: max_conns,
                                });
                            } else {
                                issues.push(ValidationIssue {
                                    path: format!("{prefix}.bind_addr"),
                                    message: "bind_addr is required"
                                        .to_string(),
                                });
                            }
                        }
                    }
                    "stream_body_threshold" => {
                        if let Some(n) = parse_positive_usize(k, v, &mut issues)
                        {
                            cfg.stream_body_threshold = n;
                        }
                    }
                    "max_request_line_bytes" => {
                        if let Some(n) = parse_positive_usize(k, v, &mut issues)
                        {
                            cfg.max_request_line_bytes = n;
                        }
                    }
                    "max_headers" => {
                        if let Some(n) = parse_positive_usize(k, v, &mut issues)
                        {
                            cfg.max_headers = n;
                        }
                    }
                    "max_header_bytes" => {
                        if let Some(n) = parse_positive_usize(k, v, &mut issues)
                        {
                            cfg.max_header_bytes = n;
                        }
                    }
                    "max_body_bytes" => {
                        if let Some(n) = parse_positive_usize(k, v, &mut issues)
                        {
                            cfg.max_body_bytes = n;
                        }
                    }
                    "idle_timeout_ms" => match v {
                        ConfigValue::Int(ms) if *ms > 0 => {
                            cfg.idle_timeout = Duration::from_millis(*ms as u64);
                        }
                        _ => issues.push(ValidationIssue {
                            path: k.clone(),
                            message: "idle_timeout_ms must be a positive integer"
                                .to_string(),
                        }),
                    },
                    _ => issues.push(ValidationIssue {
                        path: k.clone(),
                        message: format!("unknown proto-http option `{k}`"),
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
            message: "proto-http config must be Null or Map".to_string(),
        }])),
    }
}

impl Plugin for HttpEnginePlugin {
    fn name(&self) -> &str {
        "proto-http"
    }

    fn deps(&self) -> Deps {
        if self.config.bind_addr.is_some() {
            Deps::empty().with::<TransportService>()
        } else {
            Deps::empty().with_optional::<TransportService>()
        }
    }

    fn schema(&self) -> PluginSchema {
        PluginSchema::new(
            "Layer 2 bounded HTTP/1.1 parser, chunked streamer, and lock-free snapshot router",
        )
        .field(
            ConfigFieldSchema::new(
                "bind_addr",
                ConfigFieldType::Str,
                "TCP socket address to bind HTTP/1.1 listener (null runs router in-memory only)",
            )
            .nullable()
            .with_default(
                self.config
                    .bind_addr
                    .map_or(ConfigValue::Null, |a| ConfigValue::Str(a.to_string())),
            ),
        )
        .field(
            ConfigFieldSchema::new(
                "tls_listener_id",
                ConfigFieldType::Str,
                "Optional TLS listener ID registered in l4-tls for the primary bind_addr",
            )
            .nullable()
            .with_default(
                self.config
                    .tls_listener_id
                    .as_ref()
                    .map_or(ConfigValue::Null, |s| ConfigValue::Str(s.clone())),
            ),
        )
        .field(
            ConfigFieldSchema::new(
                "max_connections",
                ConfigFieldType::Int,
                "Optional concurrent connection limit for the primary HTTP listener",
            )
            .nullable()
            .with_default(
                self.config
                    .max_connections
                    .map_or(ConfigValue::Null, |n| ConfigValue::Int(n as i64)),
            ),
        )
        .field(ConfigFieldSchema::new(
            "extra_listeners",
            ConfigFieldType::List,
            "Additional HTTP/HTTPS listeners ([{bind_addr, tls_listener_id, max_connections}])",
        ))
        .field(
            ConfigFieldSchema::new(
                "stream_body_threshold",
                ConfigFieldType::Int,
                "Request bodies above this byte size switch from buffered Bytes to Stream",
            )
            .with_default(ConfigValue::Int(
                self.config.stream_body_threshold as i64,
            )),
        )
        .field(
            ConfigFieldSchema::new(
                "max_request_line_bytes",
                ConfigFieldType::Int,
                "Maximum request-line length in bytes (414 when exceeded)",
            )
            .with_default(ConfigValue::Int(
                self.config.max_request_line_bytes as i64,
            )),
        )
        .field(
            ConfigFieldSchema::new(
                "max_headers",
                ConfigFieldType::Int,
                "Maximum HTTP headers accepted per request (431 when exceeded)",
            )
            .with_default(ConfigValue::Int(self.config.max_headers as i64)),
        )
        .field(
            ConfigFieldSchema::new(
                "max_header_bytes",
                ConfigFieldType::Int,
                "Maximum total HTTP header section size in bytes (431 when exceeded)",
            )
            .with_default(ConfigValue::Int(self.config.max_header_bytes as i64)),
        )
        .field(
            ConfigFieldSchema::new(
                "max_body_bytes",
                ConfigFieldType::Int,
                "Maximum request body length in bytes (413 when exceeded)",
            )
            .with_default(ConfigValue::Int(self.config.max_body_bytes as i64)),
        )
        .field(
            ConfigFieldSchema::new(
                "idle_timeout_ms",
                ConfigFieldType::Int,
                "Keep-alive connection idle timeout in milliseconds",
            )
            .with_default(ConfigValue::Int(
                self.config.idle_timeout.as_millis() as i64,
            )),
        )
    }

    fn validate_config(
        &self,
        raw: &ConfigValue,
    ) -> Result<ConfigValue, FrameError> {
        let _ = resolve_http_config(&self.config, raw)?;
        Ok(raw.clone())
    }

    fn apply(
        &self,
        ctx: Context,
        config: ConfigValue,
    ) -> BoxFuture<'static, Result<(), FrameError>> {
        let base = self.config.clone();
        Box::pin(async move {
            let merged = ctx.resolve_intercept::<RouterService>(config);
            let config = resolve_http_config(&base, &merged)?;

            let router = HttpRouter::new();
            ctx.provide(Arc::new(RouterService::new(
                Arc::clone(&router) as Arc<dyn http_api::Router>
            )))
            .await?;

            if let Some(bind_addr) = config.bind_addr {
                let transport =
                    ctx.try_use_service::<TransportService>().ok_or_else(|| {
                        FrameError::new(
                            ErrorKind::MissingDependency,
                            "TransportService is required when bind_addr is set",
                        )
                    })?;
                let driver: Arc<dyn ConnectionHandler> =
                    Arc::new(HttpConnectionDriver {
                        router: Arc::clone(&router),
                        config: config.clone(),
                    });
                let (local_addr, _listener_handle) = transport
                    .listen_with_options(
                        ListenOptions {
                            addr: bind_addr,
                            max_connections: config.max_connections,
                            tls_listener_id: config.tls_listener_id.clone(),
                        },
                        driver,
                    )
                    .await?;
                ctx.provide(Arc::new(BoundServerInfo { local_addr }))
                    .await?;
            }

            if !config.extra_listeners.is_empty() {
                let transport =
                    ctx.try_use_service::<TransportService>().ok_or_else(|| {
                        FrameError::new(
                            ErrorKind::MissingDependency,
                            "TransportService is required when extra_listeners is set",
                        )
                    })?;
                let driver: Arc<dyn ConnectionHandler> =
                    Arc::new(HttpConnectionDriver {
                        router: Arc::clone(&router),
                        config: config.clone(),
                    });
                for extra in &config.extra_listeners {
                    let _ = transport
                        .listen_with_options(
                            ListenOptions {
                                addr: extra.bind_addr,
                                max_connections: extra.max_connections,
                                tls_listener_id: extra.tls_listener_id.clone(),
                            },
                            Arc::clone(&driver),
                        )
                        .await?;
                }
            }

            Ok(())
        })
    }
}
