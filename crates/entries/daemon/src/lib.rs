//! Component-agnostic daemon runtime host for `d` assemblies.
//!
//! Coordinates Layer 0 (`frame`), contract crates, CLI parsing, and signal
//! lifecycle without depending directly on any concrete plugin components.

use d_cli::ParseOutcome;
use frame::{
    BoxFuture, Context, ErrorKind, FrameError, FrameOptions, FrameSnapshot,
    Loader, PluginEntry, Resolver, Spawner,
};
use http_api::{BoundServerInfo, HttpRequest, HttpResponse, RouterService};
use std::io::Write;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::exit;
use std::sync::Arc;

/// Tokio-backed task spawner for the microkernel runtime.
#[derive(Debug, Default, Clone, Copy)]
pub struct DaemonSpawner;

impl Spawner for DaemonSpawner {
    fn spawn(&self, fut: BoxFuture<'static, ()>) {
        tokio::spawn(fut);
    }
}

/// Shared configuration toggles passed from entry shells into assemblies.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DaemonOptions {
    pub allow_hidden: bool,
    pub cors: bool,
}

/// Declarative assembly blueprint built by a concrete binary or test harness.
pub struct AssemblySpec {
    pub resolver: Arc<dyn Resolver>,
    pub entries: Vec<PluginEntry>,
}

/// Live assembled daemon handle supporting in-memory dispatch, inspection,
/// incremental `Loader` patching, and graceful shutdown.
pub struct AppHandle {
    ctx: Context,
    loader: Loader,
    router: RouterService,
    local_addr: Option<SocketAddr>,
}

impl AppHandle {
    /// Borrow the Layer 2 [`RouterService`].
    pub fn router(&self) -> &RouterService {
        &self.router
    }

    /// Borrow the Layer 0 plugin [`Context`].
    pub fn context(&self) -> &Context {
        &self.ctx
    }

    /// Borrow the declarative [`Loader`] managing this assembly's plugin tree.
    pub fn loader(&self) -> &Loader {
        &self.loader
    }

    /// Capture a diagnostic runtime snapshot from the microkernel.
    pub fn inspect(&self) -> FrameSnapshot {
        self.ctx.inspect()
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

    /// Gracefully close the loader, dispose the plugin tree, and drain transport listeners.
    pub async fn dispose(self) -> Result<(), FrameError> {
        let _ = self.loader.close().await;
        self.ctx.dispose().await
    }
}

/// Instantiate and settle an [`AppHandle`] from a declarative [`AssemblySpec`].
pub async fn build_from_spec(
    spec: AssemblySpec,
    expect_listener: bool,
) -> Result<AppHandle, FrameError> {
    let opts = FrameOptions::new(Arc::new(DaemonSpawner)).with_on_error(
        Arc::new(|err| {
            d_cli::log_error(&format!("runtime frame error: {err}"));
        }),
    );
    let ctx = Context::new(opts);
    let loader = Loader::new(ctx.clone(), spec.resolver);
    loader.apply(spec.entries).await?;
    ctx.settle().await;
    ctx.mark_ready();

    let router = ctx
        .get::<RouterService>()
        .ok_or_else(|| {
            FrameError::new(
                ErrorKind::MissingDependency,
                "failed to initialize HTTP router service",
            )
        })?
        .as_ref()
        .clone();

    let local_addr = if expect_listener {
        Some(
            ctx.get::<BoundServerInfo>()
                .ok_or_else(|| {
                    FrameError::new(
                        ErrorKind::MissingDependency,
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
        loader,
        router,
        local_addr,
    })
}

/// Start a daemon from an assembly builder on `addr` serving `root` until
/// `SIGINT` or `SIGTERM`.
pub async fn start_from_spec<F>(
    addr: &SocketAddr,
    root: &str,
    options: DaemonOptions,
    build_spec: F,
) where
    F: FnOnce(PathBuf, DaemonOptions, Option<SocketAddr>) -> AssemblySpec,
{
    let canonical = match std::fs::canonicalize(root) {
        Ok(p) => p,
        Err(e) => {
            d_cli::log_error(&format!("Invalid root path '{root}': {e}"));
            exit(1);
        }
    };

    let spec = build_spec(canonical.clone(), options, Some(*addr));
    let app = match build_from_spec(spec, true).await {
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

/// Parse CLI/environment options, initialize stderr logging, and run a daemon assembly.
pub async fn run_cli_daemon<F>(banner: &str, build_spec: F)
where
    F: FnOnce(PathBuf, DaemonOptions, Option<SocketAddr>) -> AssemblySpec,
{
    let cfg = match d_cli::parse_from_env() {
        ParseOutcome::Run(cfg) => cfg,
        ParseOutcome::ExitOk(msg) => {
            print!("{msg}");
            return;
        }
        ParseOutcome::ExitErr(err) => {
            eprintln!("Error: {err}");
            exit(2);
        }
    };

    d_cli::init_log_level(cfg.log_level);

    let root_str = cfg.root.to_string_lossy().to_string();
    d_cli::log_info(banner);
    d_cli::log_info("Configuration:");
    d_cli::log_info(&format!("  Host: {}", cfg.host));
    d_cli::log_info(&format!("  Port: {}", cfg.port));
    d_cli::log_info(&format!("  Root: {root_str}"));
    d_cli::log_info(&format!("  Log:  {}", cfg.log_level.as_str()));
    d_cli::log_info(&format!(
        "  Hidden files: {}",
        if cfg.allow_hidden {
            "allowed"
        } else {
            "disabled"
        }
    ));
    d_cli::log_info(&format!(
        "  CORS: {}",
        if cfg.cors { "permissive" } else { "disabled" }
    ));

    start_from_spec(
        &cfg.addr,
        &root_str,
        DaemonOptions {
            allow_hidden: cfg.allow_hidden,
            cors: cfg.cors,
        },
        build_spec,
    )
    .await;
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
