//! Root assembly for `d`: wires Layer 0 microkernel (`frame`) together with
//! L4 transport, HTTP/1.1 engine, extensions, and file application plugins.

use app_files::{FilesAppPlugin, FilesConfig};
use ext_compression::CompressionPlugin;
use ext_cors::CorsPlugin;
use frame::{BoxFuture, ConfigValue, Context, FrameError, FrameOptions};
use http_api::{HttpRequest, HttpResponse, RouterService};
use l4_transport::{TokioSpawner, TransportPlugin};
use proto_http::{BoundServerInfo, HttpConfig, HttpEnginePlugin};
use std::io::Write;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::exit;
use std::sync::Arc;

/// Configuration toggles for assembling the `d` HTTP server runtime.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Options {
    pub allow_hidden: bool,
    pub cors: bool,
}

/// Live assembled `d` runtime handle supporting in-memory dispatch and shutdown.
pub struct AppHandle {
    ctx: Context,
    router: RouterService,
    local_addr: Option<SocketAddr>,
}

impl AppHandle {
    /// Borrow the underlying Layer 2 [`RouterService`].
    pub fn router(&self) -> &RouterService {
        &self.router
    }

    /// Borrow the Layer 0 plugin [`Context`].
    pub fn context(&self) -> &Context {
        &self.ctx
    }

    /// Return the actual bound TCP socket address when network listening is enabled.
    pub fn local_addr(&self) -> Option<SocketAddr> {
        self.local_addr
    }

    /// Dispatch an HTTP request directly through the assembled filter and route snapshot.
    pub fn dispatch(
        &self,
        req: HttpRequest,
    ) -> BoxFuture<'static, HttpResponse> {
        self.router.dispatch(req)
    }

    /// Gracefully dispose the plugin tree and drain any active transport listeners.
    pub async fn dispose(self) -> Result<(), FrameError> {
        self.ctx.dispose().await
    }
}

/// Assemble the `d` plugin tree for `root` with optional TCP listener binding.
pub async fn build_runtime(
    root: PathBuf,
    options: Options,
    bind_addr: Option<SocketAddr>,
) -> Result<AppHandle, FrameError> {
    let opts = FrameOptions::new(Arc::new(TokioSpawner)).with_on_error(
        Arc::new(|err| {
            d_cli::log_error(&format!("runtime frame error: {err}"));
        }),
    );
    let ctx = Context::new(opts);

    if bind_addr.is_some() {
        ctx.plugin(TransportPlugin::default(), ConfigValue::Null)?;
    }

    ctx.plugin(
        HttpEnginePlugin::new(HttpConfig {
            bind_addr,
            ..HttpConfig::default()
        }),
        ConfigValue::Null,
    )?;

    ctx.plugin(CompressionPlugin::default(), ConfigValue::Null)?;

    if options.cors {
        ctx.plugin(CorsPlugin, ConfigValue::Null)?;
    }

    ctx.plugin(
        FilesAppPlugin::new(FilesConfig::new(root, options.allow_hidden)),
        ConfigValue::Null,
    )?;

    ctx.settle().await;

    let router = ctx
        .get::<RouterService>()
        .ok_or_else(|| {
            FrameError::new(
                frame::ErrorKind::MissingDependency,
                "failed to initialize HTTP router service",
            )
        })?
        .as_ref()
        .clone();

    let local_addr = if bind_addr.is_some() {
        Some(
            ctx.get::<BoundServerInfo>()
                .ok_or_else(|| {
                    FrameError::new(
                        frame::ErrorKind::MissingDependency,
                        "failed to bind HTTP listener",
                    )
                })?
                .local_addr,
        )
    } else {
        None
    };

    Ok(AppHandle {
        ctx,
        router,
        local_addr,
    })
}

/// Construct an in-memory HTTP application serving `root` with default options.
pub async fn create_app(
    root: impl AsRef<Path>,
    allow_hidden: bool,
) -> AppHandle {
    create_app_with_options(
        root,
        Options {
            allow_hidden,
            cors: false,
        },
    )
    .await
}

/// Construct an in-memory HTTP application serving `root` with explicit [`Options`].
pub async fn create_app_with_options(
    root: impl AsRef<Path>,
    options: Options,
) -> AppHandle {
    let path = root.as_ref().to_path_buf();
    let canonical = path.canonicalize().unwrap_or(path);
    build_runtime(canonical, options, None)
        .await
        .expect("in-memory d runtime assembly must succeed")
}

/// Start the HTTP server on `addr` serving `root` with default options.
pub async fn start(addr: &SocketAddr, root: &str) {
    start_with_options(addr, root, Options::default()).await;
}

/// Start the HTTP server on `addr` serving `root` until `SIGINT` or `SIGTERM`.
pub async fn start_with_options(
    addr: &SocketAddr,
    root: &str,
    options: Options,
) {
    let canonical = match std::fs::canonicalize(root) {
        Ok(p) => p,
        Err(e) => {
            d_cli::log_error(&format!("Invalid root path '{root}': {e}"));
            exit(1);
        }
    };

    let app = match build_runtime(canonical.clone(), options, Some(*addr)).await
    {
        Ok(app) => app,
        Err(e) => {
            d_cli::log_error(&format!("Failed to start server on {addr}: {e}"));
            exit(1);
        }
    };

    if let Some(bound) = app.local_addr() {
        d_cli::log_info(&format!(
            "Server started successfully on http://{bound}"
        ));
    }
    d_cli::log_info(&format!(
        "Serving root directory: {}",
        canonical.display()
    ));
    let _ = std::io::stdout().flush();

    wait_for_shutdown_signal().await;
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();

    if let Err(e) = app.dispose().await {
        d_cli::log_error(&format!(
            "Graceful shutdown finished with errors: {e}"
        ));
    }
}

async fn wait_for_shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let sigterm = signal(SignalKind::terminate());
        match sigterm {
            Ok(mut term_stream) => {
                tokio::select! {
                    res = tokio::signal::ctrl_c() => {
                        match res {
                            Ok(()) => d_cli::log_info("Received SIGINT (Ctrl+C), shutting down"),
                            Err(e) => d_cli::log_error(&format!("Failed listening for Ctrl+C: {e}")),
                        }
                    }
                    _ = term_stream.recv() => {
                        d_cli::log_info("Received SIGTERM, shutting down");
                    }
                }
            }
            Err(e) => {
                d_cli::log_error(&format!(
                    "Failed registering SIGTERM handler: {e}"
                ));
                let _ = tokio::signal::ctrl_c().await;
                d_cli::log_info("Received SIGINT (Ctrl+C), shutting down");
            }
        }
    }

    #[cfg(not(unix))]
    {
        match tokio::signal::ctrl_c().await {
            Ok(()) => {
                d_cli::log_info(
                    "Received shutdown signal (Ctrl+C), shutting down",
                );
            }
            Err(e) => {
                d_cli::log_error(&format!(
                    "Failed to listen for shutdown signal: {e}"
                ));
            }
        }
    }
}
