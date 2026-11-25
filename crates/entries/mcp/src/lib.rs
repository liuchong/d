//! Optional, configurable pure-Rust MCP control server for `d`.
//!
//! Mounts as a Layer 0 control-plane plugin (`McpPlugin`) atop [`LoaderService`]
//! and [`RuntimeControlService`] with zero coupling to concrete components.

pub mod json;
pub mod protocol;
pub mod tools;
pub mod transport;

use d_cli::McpCliConfig;
use d_daemon::RuntimeControlService;
use frame::{
    BoxFuture, ConfigFieldSchema, ConfigFieldType, ConfigValue, Context, Deps,
    ErrorKind, FrameError, LoaderService, Plugin, PluginSchema, Service,
    ValidationIssue,
};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::{BufReader, stdin, stdout};
use tokio::sync::watch;
pub use tools::{McpServer, McpServerPolicy};
use transport::{bind_http_listener, serve_http_loop, serve_stdio};

/// Supported MCP transport modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpTransportMode {
    Stdio,
    Http,
    Both,
}

/// Configuration for [`McpPlugin`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpConfig {
    pub transport: McpTransportMode,
    pub http_addr: Option<SocketAddr>,
    pub http_path: String,
    pub auth_token: Option<String>,
    pub read_only: bool,
    pub allow_shutdown: bool,
    pub shutdown_on_eof: bool,
}

impl Default for McpConfig {
    fn default() -> Self {
        Self {
            transport: McpTransportMode::Stdio,
            http_addr: None,
            http_path: "/mcp".to_string(),
            auth_token: None,
            read_only: false,
            allow_shutdown: false,
            shutdown_on_eof: true,
        }
    }
}

impl McpConfig {
    /// Convert CLI/ENV MCP configuration into [`McpConfig`].
    pub fn from_cli(cli: &McpCliConfig) -> Option<Self> {
        let transport = match (cli.stdio, cli.http_addr.is_some()) {
            (false, false) => return None,
            (true, false) => McpTransportMode::Stdio,
            (false, true) => McpTransportMode::Http,
            (true, true) => McpTransportMode::Both,
        };
        Some(Self {
            transport,
            http_addr: cli.http_addr,
            http_path: cli.http_path.clone(),
            auth_token: cli.auth_token.clone(),
            read_only: cli.read_only,
            allow_shutdown: cli.allow_shutdown,
            shutdown_on_eof: true,
        })
    }
}

/// Runtime service recording the actual bound MCP HTTP listener address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundMcpInfo {
    pub http_addr: Option<SocketAddr>,
    pub http_path: String,
}

impl Service for BoundMcpInfo {
    const NAME: &'static str = "d.mcp.bound";
}

/// Control-plane plugin mounting stdio and/or Streamable HTTP MCP transports.
#[derive(Debug, Clone, Default)]
pub struct McpPlugin {
    config: McpConfig,
}

impl McpPlugin {
    /// Construct an [`McpPlugin`] with explicit defaults (can be overridden by `ConfigValue`).
    pub fn new(config: McpConfig) -> Self {
        Self { config }
    }
}

fn resolve_mcp_config(
    base: &McpConfig,
    raw: &ConfigValue,
) -> Result<McpConfig, FrameError> {
    let mut cfg = base.clone();
    match raw {
        ConfigValue::Null => {}
        ConfigValue::Map(map) => {
            let mut issues = Vec::new();
            for (k, v) in map {
                match k.as_str() {
                    "transport" => match v {
                        ConfigValue::Str(s) => match s.as_str() {
                            "stdio" => cfg.transport = McpTransportMode::Stdio,
                            "http" => cfg.transport = McpTransportMode::Http,
                            "both" => cfg.transport = McpTransportMode::Both,
                            _ => issues.push(ValidationIssue {
                                path: k.clone(),
                                message: "transport must be stdio, http, or both"
                                    .to_string(),
                            }),
                        },
                        _ => issues.push(ValidationIssue {
                            path: k.clone(),
                            message: "transport must be a string".to_string(),
                        }),
                    },
                    "http_addr" => match v {
                        ConfigValue::Null => cfg.http_addr = None,
                        ConfigValue::Str(s) => match s.parse::<SocketAddr>() {
                            Ok(a) => cfg.http_addr = Some(a),
                            Err(e) => issues.push(ValidationIssue {
                                path: k.clone(),
                                message: format!("invalid http_addr `{s}`: {e}"),
                            }),
                        },
                        _ => issues.push(ValidationIssue {
                            path: k.clone(),
                            message: "http_addr must be a socket address string or null"
                                .to_string(),
                        }),
                    },
                    "http_path" => match v {
                        ConfigValue::Str(s) if s.starts_with('/') => {
                            cfg.http_path = s.clone();
                        }
                        _ => issues.push(ValidationIssue {
                            path: k.clone(),
                            message: "http_path must be a string starting with '/'"
                                .to_string(),
                        }),
                    },
                    "auth_token" => match v {
                        ConfigValue::Null => cfg.auth_token = None,
                        ConfigValue::Str(s) if !s.is_empty() => {
                            cfg.auth_token = Some(s.clone());
                        }
                        _ => issues.push(ValidationIssue {
                            path: k.clone(),
                            message: "auth_token must be a non-empty string or null"
                                .to_string(),
                        }),
                    },
                    "read_only" => match v {
                        ConfigValue::Bool(b) => cfg.read_only = *b,
                        _ => issues.push(ValidationIssue {
                            path: k.clone(),
                            message: "read_only must be boolean".to_string(),
                        }),
                    },
                    "allow_shutdown" => match v {
                        ConfigValue::Bool(b) => cfg.allow_shutdown = *b,
                        _ => issues.push(ValidationIssue {
                            path: k.clone(),
                            message: "allow_shutdown must be boolean".to_string(),
                        }),
                    },
                    "shutdown_on_eof" => match v {
                        ConfigValue::Bool(b) => cfg.shutdown_on_eof = *b,
                        _ => issues.push(ValidationIssue {
                            path: k.clone(),
                            message: "shutdown_on_eof must be boolean"
                                .to_string(),
                        }),
                    },
                    _ => issues.push(ValidationIssue {
                        path: k.clone(),
                        message: format!("unknown d-mcp option `{k}`"),
                    }),
                }
            }
            if !issues.is_empty() {
                return Err(FrameError::validation(issues));
            }
        }
        _ => {
            return Err(FrameError::validation(vec![ValidationIssue {
                path: "".to_string(),
                message: "d-mcp config must be Null or Map".to_string(),
            }]));
        }
    }

    if matches!(
        cfg.transport,
        McpTransportMode::Http | McpTransportMode::Both
    ) && cfg.http_addr.is_none()
    {
        return Err(FrameError::validation(vec![ValidationIssue {
            path: "http_addr".to_string(),
            message: "http_addr is required when transport includes http"
                .to_string(),
        }]));
    }
    Ok(cfg)
}

impl Plugin for McpPlugin {
    fn name(&self) -> &str {
        "d-mcp"
    }

    fn deps(&self) -> Deps {
        Deps::empty()
            .with::<LoaderService>()
            .with::<RuntimeControlService>()
    }

    fn schema(&self) -> PluginSchema {
        PluginSchema::new(
            "Optional configurable MCP control server exposing live inspection, schema discovery, and hot reconfiguration",
        )
        .field(
            ConfigFieldSchema::new(
                "transport",
                ConfigFieldType::Str,
                "MCP transport mode: stdio, http, or both",
            )
            .with_default(ConfigValue::Str("stdio".to_string())),
        )
        .field(
            ConfigFieldSchema::new(
                "http_addr",
                ConfigFieldType::Str,
                "Socket address for Streamable HTTP MCP endpoint",
            )
            .nullable(),
        )
        .field(
            ConfigFieldSchema::new(
                "http_path",
                ConfigFieldType::Str,
                "HTTP path serving MCP JSON-RPC 2.0 POST requests",
            )
            .with_default(ConfigValue::Str("/mcp".to_string())),
        )
        .field(
            ConfigFieldSchema::new(
                "auth_token",
                ConfigFieldType::Str,
                "Optional Bearer token protecting the HTTP MCP endpoint",
            )
            .nullable()
            .sensitive(),
        )
        .field(
            ConfigFieldSchema::new(
                "read_only",
                ConfigFieldType::Bool,
                "Disable all mutating MCP tools and expose read-only inspection only",
            )
            .with_default(ConfigValue::Bool(false)),
        )
        .field(
            ConfigFieldSchema::new(
                "allow_shutdown",
                ConfigFieldType::Bool,
                "Allow the d_shutdown MCP tool to stop the daemon process",
            )
            .with_default(ConfigValue::Bool(false)),
        )
        .field(
            ConfigFieldSchema::new(
                "shutdown_on_eof",
                ConfigFieldType::Bool,
                "Request daemon shutdown when stdio input reaches EOF",
            )
            .with_default(ConfigValue::Bool(true)),
        )
    }

    fn validate_config(
        &self,
        raw: &ConfigValue,
    ) -> Result<ConfigValue, FrameError> {
        let _ = resolve_mcp_config(&self.config, raw)?;
        Ok(raw.clone())
    }

    fn apply(
        &self,
        ctx: Context,
        config: ConfigValue,
    ) -> BoxFuture<'static, Result<(), FrameError>> {
        let base = self.config.clone();
        Box::pin(async move {
            let cfg = resolve_mcp_config(&base, &config)?;
            let loader = ctx.require::<LoaderService>()?.as_ref().clone();
            let control = ctx.require::<RuntimeControlService>()?;

            let wants_http = matches!(
                cfg.transport,
                McpTransportMode::Http | McpTransportMode::Both
            );
            let wants_stdio = matches!(
                cfg.transport,
                McpTransportMode::Stdio | McpTransportMode::Both
            );

            let bound_http = if wants_http {
                let addr = cfg.http_addr.expect("validated http_addr");
                let (listener, local) =
                    bind_http_listener(addr).await.map_err(|e| {
                        FrameError::new(
                            ErrorKind::ActivationFailed,
                            format!(
                                "failed to bind MCP HTTP listener on {addr}: {e}"
                            ),
                        )
                    })?;
                Some((listener, local))
            } else {
                None
            };

            let bound_addr = bound_http.as_ref().map(|(_, addr)| *addr);
            let server = McpServer::new(
                ctx.clone(),
                loader,
                Arc::clone(&control),
                McpServerPolicy {
                    read_only: cfg.read_only,
                    allow_shutdown: cfg.allow_shutdown,
                    stdio: wants_stdio,
                    http_addr: bound_addr,
                    http_path: cfg.http_path.clone(),
                    auth_token_configured: cfg.auth_token.is_some(),
                },
            );

            let (stop_tx, stop_rx) = watch::channel(false);
            let mut handles = Vec::new();

            if let Some((listener, _local)) = bound_http {
                let srv = server.clone();
                let path = cfg.http_path.clone();
                let tok = cfg.auth_token.clone().map(Arc::new);
                let rx = stop_rx.clone();
                handles.push(tokio::spawn(async move {
                    serve_http_loop(srv, listener, path, tok, rx).await;
                }));
            }

            if wants_stdio {
                let srv = server;
                let ctrl = Arc::clone(&control);
                let shutdown_on_eof = cfg.shutdown_on_eof;
                let rx = stop_rx.clone();
                handles.push(tokio::spawn(async move {
                    let reader = BufReader::new(stdin());
                    let writer = stdout();
                    serve_stdio(srv, reader, writer, rx, move || {
                        if shutdown_on_eof {
                            ctrl.request_shutdown();
                        }
                    })
                    .await;
                }));
            }

            ctx.effect("mcp-transports", move |scope| {
                Box::pin(async move {
                    scope.defer_async(move || {
                        Box::pin(async move {
                            let _ = stop_tx.send(true);
                            for h in handles {
                                let _ = h.await;
                            }
                            Ok(())
                        })
                    });
                    Ok(())
                })
            })
            .await?;

            ctx.provide(Arc::new(BoundMcpInfo {
                http_addr: bound_addr,
                http_path: cfg.http_path,
            }))
            .await?;

            Ok(())
        })
    }
}

/// Convenience helper mounting [`McpPlugin`] onto an assembled runtime [`Context`]
/// from parsed [`McpCliConfig`].
pub fn attach_mcp(
    ctx: Context,
    cli: McpCliConfig,
) -> BoxFuture<'static, Result<(), FrameError>> {
    Box::pin(async move {
        let Some(cfg) = McpConfig::from_cli(&cli) else {
            return Ok(());
        };
        let fiber = ctx.plugin(McpPlugin::new(cfg), ConfigValue::Null)?;
        fiber.wait().await
    })
}
