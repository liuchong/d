//! Component-agnostic daemon runtime host for `d` assemblies.
//!
//! Coordinates Layer 0 (`frame`), contract crates, CLI parsing, and signal
//! lifecycle without depending directly on any concrete plugin components.

pub mod config_json;

pub use config_json::{load_entries_from_json_file, parse_entries_from_json};
use d_cli::{LogLevel, McpCliConfig, ParseOutcome};
use frame::{
    BoxFuture, Context, ErrorKind, FrameError, FrameOptions, FrameSnapshot,
    Loader, LoaderService, PluginEntry, Resolver, Service, Spawner,
};
use http_api::{BoundServerInfo, HttpRequest, HttpResponse, RouterService};
use std::future::Future;
use std::io::Write;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::exit;
use std::sync::Arc;
use tokio::sync::watch;

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

/// Live control-plane service exposed on the runtime [`Context`] for control
/// plugins (such as `d-mcp`) to inspect live addresses, dispatch in-memory HTTP
/// probes, change process logging verbosity, or trigger graceful shutdown.
pub struct RuntimeControlService {
    assembly_name: String,
    version: String,
    ctx: Context,
    shutdown_tx: watch::Sender<bool>,
}

impl RuntimeControlService {
    /// Assembly binary/distribution identifier (e.g. `"d"` or `"d-minimal"`).
    pub fn assembly_name(&self) -> &str {
        &self.assembly_name
    }

    /// Package version of the daemon host.
    pub fn version(&self) -> &str {
        &self.version
    }

    /// Current global log threshold.
    pub fn log_level(&self) -> LogLevel {
        d_cli::current_log_level()
    }

    /// Dynamically adjust the process-wide logging verbosity threshold.
    pub fn set_log_level(&self, level: LogLevel) {
        d_cli::init_log_level(level);
    }

    /// Query the currently bound HTTP TCP socket address, if a listener is active.
    pub fn local_addr(&self) -> Option<SocketAddr> {
        self.ctx.get::<BoundServerInfo>().map(|b| b.local_addr)
    }

    /// Dispatch a synthetic HTTP request through the currently active [`RouterService`].
    pub fn dispatch_http(
        &self,
        req: HttpRequest,
    ) -> BoxFuture<'static, Result<HttpResponse, FrameError>> {
        let router = self.ctx.get::<RouterService>();
        Box::pin(async move {
            let Some(router) = router else {
                return Err(FrameError::new(
                    ErrorKind::MissingDependency,
                    "HTTP router service is not currently active",
                ));
            };
            Ok(router.dispatch(req).await)
        })
    }

    /// Request graceful daemon shutdown (caught alongside `SIGINT`/`SIGTERM`).
    pub fn request_shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
    }

    /// Check whether graceful shutdown has already been requested.
    pub fn shutdown_requested(&self) -> bool {
        *self.shutdown_tx.borrow()
    }

    /// Return `true` when all active fibers are healthy (zero `Failed` fibers).
    pub fn readiness_ok(&self) -> bool {
        !self
            .ctx
            .inspect()
            .fibers
            .iter()
            .any(|f| matches!(f.state, frame::FiberState::Failed))
    }
}

impl Service for RuntimeControlService {
    const NAME: &'static str = "d.runtime.control";
}

/// Live assembled daemon handle supporting in-memory dispatch, inspection,
/// incremental `Loader` patching, live control, and graceful shutdown.
pub struct AppHandle {
    ctx: Context,
    loader: Loader,
    router: RouterService,
    control: Arc<RuntimeControlService>,
    shutdown_rx: watch::Receiver<bool>,
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

    /// Borrow the live [`RuntimeControlService`].
    pub fn control(&self) -> &Arc<RuntimeControlService> {
        &self.control
    }

    /// Capture a diagnostic runtime snapshot from the microkernel.
    pub fn inspect(&self) -> FrameSnapshot {
        self.ctx.inspect()
    }

    /// Return the live bound TCP socket address when network listening is enabled.
    pub fn local_addr(&self) -> Option<SocketAddr> {
        self.control.local_addr()
    }

    /// Dispatch an HTTP request directly through the live filter and route snapshot.
    pub fn dispatch(
        &self,
        req: HttpRequest,
    ) -> BoxFuture<'static, HttpResponse> {
        if let Some(router) = self.ctx.get::<RouterService>() {
            router.dispatch(req)
        } else {
            self.router.dispatch(req)
        }
    }

    /// Wait until programmatic shutdown is requested via [`RuntimeControlService::request_shutdown`].
    pub async fn wait_for_control_shutdown(&mut self) {
        while !*self.shutdown_rx.borrow() {
            if self.shutdown_rx.changed().await.is_err() {
                break;
            }
        }
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
    build_from_spec_named("d", spec, expect_listener).await
}

/// Instantiate and settle a named [`AppHandle`] from a declarative [`AssemblySpec`].
pub async fn build_from_spec_named(
    assembly_name: &str,
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
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let control = Arc::new(RuntimeControlService {
        assembly_name: assembly_name.to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        ctx: ctx.clone(),
        shutdown_tx,
    });

    ctx.provide(Arc::new(LoaderService::new(loader.clone())))
        .await?;
    ctx.provide(Arc::clone(&control)).await?;

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

    if expect_listener && ctx.get::<BoundServerInfo>().is_none() {
        return Err(FrameError::new(
            ErrorKind::MissingDependency,
            "failed to bind HTTP listener",
        ));
    }

    Ok(AppHandle {
        ctx,
        loader,
        router,
        control,
        shutdown_rx,
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
    let mut app = match build_from_spec(spec, true).await {
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

    run_signal_loop(&mut app, None).await;
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();

    if let Err(e) = app.dispose().await {
        d_cli::log_error(&format!(
            "Graceful shutdown finished with errors: {e}"
        ));
    }
}

/// Parse CLI/environment options, initialize logging, and run a daemon assembly
/// without control-plane extensions.
pub async fn run_cli_daemon<F>(banner: &str, build_spec: F)
where
    F: FnOnce(PathBuf, DaemonOptions, Option<SocketAddr>) -> AssemblySpec,
{
    run_cli_daemon_with_control(banner, "d", build_spec, |_, _| {
        Box::pin(async { Ok(()) })
    })
    .await;
}

/// Parse CLI/environment options, initialize logging, build an assembly, optionally
/// attach control-plane extensions (such as `d-mcp`), and run until `SIGINT`,
/// `SIGTERM`, or programmatic [`RuntimeControlService::request_shutdown`].
pub async fn run_cli_daemon_with_control<F, M, Fut>(
    banner: &str,
    assembly_name: &str,
    build_spec: F,
    attach_control: M,
) where
    F: FnOnce(PathBuf, DaemonOptions, Option<SocketAddr>) -> AssemblySpec,
    M: FnOnce(Context, McpCliConfig) -> Fut,
    Fut: Future<Output = Result<(), FrameError>>,
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

    if cfg.mcp.stdio {
        d_cli::set_log_to_stderr(true);
    }
    d_cli::init_log_level(cfg.log_level);

    let root_str = cfg.root.to_string_lossy().to_string();
    d_cli::log_info(banner);
    d_cli::log_info("Configuration:");
    if cfg.no_http {
        d_cli::log_info("  HTTP: disabled initially (--no-http)");
    } else {
        d_cli::log_info(&format!("  Host: {}", cfg.host));
        d_cli::log_info(&format!("  Port: {}", cfg.port));
    }
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
    if cfg.mcp.is_enabled() {
        d_cli::log_info(&format!(
            "  MCP:  stdio={} http={} path={} read_only={}",
            cfg.mcp.stdio,
            cfg.mcp
                .http_addr
                .map_or_else(|| "off".to_string(), |a| a.to_string()),
            cfg.mcp.http_path,
            cfg.mcp.read_only
        ));
    }

    let bind_addr = if cfg.no_http { None } else { Some(cfg.addr) };
    let mut spec = build_spec(
        cfg.root.clone(),
        DaemonOptions {
            allow_hidden: cfg.allow_hidden,
            cors: cfg.cors,
        },
        bind_addr,
    );
    let mut expect_listener = !cfg.no_http;
    if let Some(ref cfg_path) = cfg.config_file {
        match load_entries_from_json_file(cfg_path) {
            Ok(entries) => {
                spec.entries = entries;
                expect_listener = false;
                d_cli::log_info(&format!("  Config: {}", cfg_path.display()));
            }
            Err(e) => {
                d_cli::log_error(&format!(
                    "Failed loading config manifest `{}`: {e}",
                    cfg_path.display()
                ));
                exit(1);
            }
        }
    }

    let mut app =
        match build_from_spec_named(assembly_name, spec, expect_listener).await
        {
            Ok(app) => app,
            Err(e) => {
                d_cli::log_error(&format!("Failed to start daemon: {e}"));
                exit(1);
            }
        };

    if cfg.mcp.is_enabled()
        && let Err(e) = attach_control(app.context().clone(), cfg.mcp).await
    {
        d_cli::log_error(&format!("Failed to start MCP control plane: {e}"));
        let _ = app.dispose().await;
        exit(1);
    }

    if let Some(bound) = app.local_addr() {
        d_cli::log_info(&format!(
            "Server started successfully on http://{bound}"
        ));
    }
    if cfg.config_file.is_none() {
        d_cli::log_info(&format!(
            "Serving root directory: {}",
            cfg.root.display()
        ));
    }
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();

    run_signal_loop(&mut app, cfg.config_file.as_deref()).await;
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();

    if let Err(e) = app.dispose().await {
        d_cli::log_error(&format!(
            "Graceful shutdown finished with errors: {e}"
        ));
    }
}

#[cfg(unix)]
async fn reload_config_manifest(app: &AppHandle, path: &std::path::Path) {
    let entries = match load_entries_from_json_file(path) {
        Ok(e) => e,
        Err(err) => {
            d_cli::log_error(&format!(
                "SIGHUP reload rejected while reading `{}`: {err}",
                path.display()
            ));
            return;
        }
    };
    if let Err(err) = app.loader().validate_entries(&entries) {
        d_cli::log_error(&format!(
            "SIGHUP reload rejected during preflight validation (`{}`): {err}",
            path.display()
        ));
        return;
    }
    match app.loader().apply(entries).await {
        Ok(_applied) => {
            app.context().settle().await;
            d_cli::log_info(&format!(
                "Reloaded config manifest from `{}` cleanly",
                path.display()
            ));
        }
        Err(err) => {
            d_cli::log_error(&format!(
                "SIGHUP reload failed while applying `{}`: {err}",
                path.display()
            ));
        }
    }
}

async fn run_signal_loop(
    app: &mut AppHandle,
    config_file: Option<&std::path::Path>,
) {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut term = signal(SignalKind::terminate()).ok();
        let mut hup = signal(SignalKind::hangup()).ok();
        loop {
            tokio::select! {
                res = tokio::signal::ctrl_c() => {
                    match res {
                        Ok(()) => d_cli::log_info("Received SIGINT (Ctrl+C), shutting down"),
                        Err(e) => d_cli::log_error(&format!("Failed listening for Ctrl+C: {e}")),
                    }
                    break;
                }
                _ = async {
                    if let Some(ref mut s) = term {
                        s.recv().await;
                    } else {
                        std::future::pending::<()>().await;
                    }
                } => {
                    d_cli::log_info("Received SIGTERM, shutting down");
                    break;
                }
                _ = async {
                    if let Some(ref mut s) = hup {
                        s.recv().await;
                    } else {
                        std::future::pending::<()>().await;
                    }
                } => {
                    if let Some(path) = config_file {
                        reload_config_manifest(app, path).await;
                    } else {
                        d_cli::log_warn("Received SIGHUP without --config manifest; ignoring reload");
                    }
                }
                _ = app.wait_for_control_shutdown() => {
                    d_cli::log_info("Received control shutdown request, shutting down");
                    break;
                }
            }
        }
    }

    #[cfg(not(unix))]
    {
        let _ = config_file;
        tokio::select! {
            res = tokio::signal::ctrl_c() => {
                match res {
                    Ok(()) => d_cli::log_info("Received shutdown signal (Ctrl+C), shutting down"),
                    Err(e) => d_cli::log_error(&format!("Failed to listen for shutdown signal: {e}")),
                }
            }
            _ = app.wait_for_control_shutdown() => {
                d_cli::log_info("Received control shutdown request, shutting down");
            }
        }
    }
}
