//! Layer 2 self-built HTTP/1.1 engine and lock-free snapshot router.

pub mod parser;
pub mod router;
pub mod writer;

use frame::{
    BoxFuture, ConfigValue, Context, Deps, EffectHandle, ErrorKind, FrameError,
    Plugin, Service,
};
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

/// Bound listener metadata published once [`HttpEnginePlugin`] opens a socket.
#[derive(Debug, Clone)]
pub struct BoundServerInfo {
    pub local_addr: SocketAddr,
}

impl Service for BoundServerInfo {
    const NAME: &'static str = "http.bound_info";
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

impl Plugin for HttpEnginePlugin {
    fn name(&self) -> &str {
        "proto-http"
    }

    fn deps(&self) -> Deps {
        if self.config.bind_addr.is_some() {
            Deps::empty().with::<TransportService>()
        } else {
            Deps::empty()
        }
    }

    fn apply(
        &self,
        ctx: Context,
        _config: ConfigValue,
    ) -> BoxFuture<'static, Result<(), FrameError>> {
        let config = self.config.clone();
        Box::pin(async move {
            let router = HttpRouter::new(ctx.clone());
            ctx.provide(Arc::new(RouterService::new(
                Arc::clone(&router) as Arc<dyn http_api::Router>
            )))
            .await?;

            if let Some(bind_addr) = config.bind_addr {
                let transport = ctx.require::<TransportService>()?;
                let driver: Arc<dyn ConnectionHandler> =
                    Arc::new(HttpConnectionDriver {
                        router: Arc::clone(&router),
                        config: config.clone(),
                    });
                let (local_addr, listener_handle) =
                    transport.listen(bind_addr, driver).await?;
                let _: EffectHandle = ctx
                    .effect("http-listener", move |scope| {
                        Box::pin(async move {
                            scope.defer_async(move || {
                                Box::pin(async move {
                                    listener_handle.dispose().await
                                })
                            });
                            Ok(())
                        })
                    })
                    .await
                    .map_err(|e| {
                        FrameError::new(ErrorKind::ActivationFailed, e.message)
                    })?;
                ctx.provide(Arc::new(BoundServerInfo { local_addr }))
                    .await?;
            }

            Ok(())
        })
    }
}
