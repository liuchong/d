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
    BoxStreamIo, ConnectionHandler, ConnectionMeta, DrainSignal,
    TransportService,
};
use parser::ConnReader;
pub use router::HttpRouter;
use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use writer::write_response;

/// HTTP/1.1 parser, listener, and connection settings.
#[derive(Debug, Clone)]
pub struct HttpConfig {
    pub bind_addr: Option<SocketAddr>,
    pub max_request_line_bytes: usize,
    pub max_headers: usize,
    pub max_header_bytes: usize,
    pub max_body_bytes: usize,
    pub idle_timeout: Duration,
}

impl Default for HttpConfig {
    fn default() -> Self {
        Self {
            bind_addr: None,
            max_request_line_bytes: 8 * 1024,
            max_headers: 128,
            max_header_bytes: 64 * 1024,
            max_body_bytes: 64 * 1024 * 1024,
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
                    .read_request(&config, meta.remote_addr, &drain)
                    .await
                {
                    Ok(Some(parsed)) => {
                        let version = parsed.request.version;
                        let is_head =
                            parsed.request.method == http_api::Method::Head;
                        let keep_alive = parsed.keep_alive;
                        let resp = <HttpRouter as http_api::Router>::dispatch(
                            &router,
                            parsed.request,
                        )
                        .await;
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
                            Ok(true) => continue,
                            Ok(false) | Err(_) => break,
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
                let (local_addr, _listener_handle) =
                    transport.listen(bind_addr, driver).await?;
                ctx.provide(Arc::new(BoundServerInfo { local_addr }))
                    .await?;
            }

            Ok(())
        })
    }
}
