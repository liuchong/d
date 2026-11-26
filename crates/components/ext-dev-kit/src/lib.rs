//! Layer 3 first-class development workflow plugin:
//! declarative HTTP/SSE Mocks, zero-CDN Live Reload, and white `/_d/inspect` UI.

pub mod inspector;
pub mod livereload;
pub mod mock;

use frame::{
    BoxFuture, ConfigFieldSchema, ConfigFieldType, ConfigValue, Context, Deps,
    FrameError, Plugin, PluginSchema, ValidationIssue,
};
use http_api::{Method, RouteMatch, RouteSpec, RouterService};
use inspector::{
    InspectEventsJsonHandler, InspectFilter, InspectReplayHandler,
    InspectStore, InspectUiHandler,
};
use livereload::{
    HtmlReloadInjector, LiveReloadConfig, LiveReloadHub, LiveReloadWsHandler,
    spawn_dir_watcher,
};
use mock::{MockHandler, MockRule, MockSseChunk};
use std::path::PathBuf;
use std::sync::Arc;
use ws_api::{WsHandler, WsRegistryService};

/// Configuration for [`DevKitPlugin`].
#[derive(Debug, Clone)]
pub struct DevKitConfig {
    pub mocks: Vec<MockRule>,
    pub live_reload: LiveReloadConfig,
    pub inspect_enabled: bool,
    pub inspect_path: String,
    pub inspect_max_entries: usize,
}

impl Default for DevKitConfig {
    fn default() -> Self {
        Self {
            mocks: Vec::new(),
            live_reload: LiveReloadConfig::default(),
            inspect_enabled: true,
            inspect_path: "/_d/inspect".to_string(),
            inspect_max_entries: 200,
        }
    }
}

/// Layer 3 plugin mounting dev-workflow Mocks, Live Reload, and `/_d/inspect`.
#[derive(Debug, Clone, Default)]
pub struct DevKitPlugin {
    config: DevKitConfig,
}

impl DevKitPlugin {
    /// Construct a dev-kit plugin with programmatic configuration.
    pub fn new(config: DevKitConfig) -> Self {
        Self { config }
    }
}

fn cfg_str(v: &ConfigValue) -> Option<&str> {
    match v {
        ConfigValue::Str(s) => Some(s.as_str()),
        _ => None,
    }
}

fn cfg_i64(v: &ConfigValue) -> Option<i64> {
    match v {
        ConfigValue::Int(n) => Some(*n),
        _ => None,
    }
}

fn cfg_bool(v: &ConfigValue) -> Option<bool> {
    match v {
        ConfigValue::Bool(b) => Some(*b),
        _ => None,
    }
}

fn parse_mock_rules(
    v: &ConfigValue,
    issues: &mut Vec<ValidationIssue>,
) -> Vec<MockRule> {
    let ConfigValue::List(arr) = v else {
        issues.push(ValidationIssue {
            path: "mocks".to_string(),
            message: "mocks must be a list".to_string(),
        });
        return Vec::new();
    };
    let mut out = Vec::with_capacity(arr.len());
    for (idx, item) in arr.iter().enumerate() {
        let prefix = format!("mocks[{idx}]");
        let ConfigValue::Map(m) = item else {
            issues.push(ValidationIssue {
                path: prefix,
                message: "mock entry must be a map".to_string(),
            });
            continue;
        };
        let mut id = format!("mock-{idx}");
        let mut method = Method::Get;
        let mut path = None;
        let mut status = 200u16;
        let mut headers = Vec::new();
        let mut body = String::new();
        let mut sse_chunks = Vec::new();

        for (k, val) in m {
            match k.as_str() {
                "id" => {
                    if let ConfigValue::Str(s) = val {
                        id = s.clone();
                    }
                }
                "method" => match val {
                    ConfigValue::Str(s) => {
                        match Method::from_bytes(
                            s.trim().to_ascii_uppercase().as_bytes(),
                        ) {
                            Some(parsed) => method = parsed,
                            None => issues.push(ValidationIssue {
                                path: format!("{prefix}.method"),
                                message: format!("invalid HTTP method `{s}`"),
                            }),
                        }
                    }
                    _ => issues.push(ValidationIssue {
                        path: format!("{prefix}.method"),
                        message: "method must be a string".to_string(),
                    }),
                },
                "path" => match val {
                    ConfigValue::Str(s) if s.starts_with('/') => {
                        path = Some(s.clone());
                    }
                    _ => issues.push(ValidationIssue {
                        path: format!("{prefix}.path"),
                        message: "path must start with `/`".to_string(),
                    }),
                },
                "status" => match val {
                    ConfigValue::Int(n) if (100..=599).contains(n) => {
                        status = *n as u16;
                    }
                    _ => issues.push(ValidationIssue {
                        path: format!("{prefix}.status"),
                        message: "status must be 100..=599".to_string(),
                    }),
                },
                "headers" => match val {
                    ConfigValue::Map(hm) => {
                        for (hk, hv) in hm {
                            if let ConfigValue::Str(s) = hv {
                                headers.push((hk.clone(), s.clone()));
                            }
                        }
                    }
                    _ => issues.push(ValidationIssue {
                        path: format!("{prefix}.headers"),
                        message: "headers must be a map".to_string(),
                    }),
                },
                "body" => match val {
                    ConfigValue::Str(s) => body = s.clone(),
                    _ => issues.push(ValidationIssue {
                        path: format!("{prefix}.body"),
                        message: "body must be a string".to_string(),
                    }),
                },
                "sse_chunks" => match val {
                    ConfigValue::List(chunks) => {
                        for (cidx, cv) in chunks.iter().enumerate() {
                            let ConfigValue::Map(cm) = cv else {
                                issues.push(ValidationIssue {
                                    path: format!(
                                        "{prefix}.sse_chunks[{cidx}]"
                                    ),
                                    message: "sse_chunk must be a map"
                                        .to_string(),
                                });
                                continue;
                            };
                            let data = cm
                                .get("data")
                                .and_then(cfg_str)
                                .unwrap_or("")
                                .to_string();
                            let delay_ms = cm
                                .get("delay_ms")
                                .and_then(cfg_i64)
                                .unwrap_or(0)
                                .max(0)
                                as u64;
                            sse_chunks.push(MockSseChunk { data, delay_ms });
                        }
                    }
                    _ => issues.push(ValidationIssue {
                        path: format!("{prefix}.sse_chunks"),
                        message: "sse_chunks must be a list".to_string(),
                    }),
                },
                _ => issues.push(ValidationIssue {
                    path: format!("{prefix}.{k}"),
                    message: format!("unknown mock field `{k}`"),
                }),
            }
        }

        if let Some(path) = path {
            out.push(MockRule {
                id,
                method,
                path,
                status,
                headers,
                body,
                sse_chunks,
            });
        } else {
            issues.push(ValidationIssue {
                path: format!("{prefix}.path"),
                message: "mock path is required".to_string(),
            });
        }
    }
    out
}

fn resolve_dev_kit_config(
    base: &DevKitConfig,
    raw: &ConfigValue,
) -> Result<DevKitConfig, FrameError> {
    let mut cfg = base.clone();
    match raw {
        ConfigValue::Null => Ok(cfg),
        ConfigValue::Map(map) => {
            let mut issues = Vec::new();
            for (k, v) in map {
                match k.as_str() {
                    "mocks" => {
                        cfg.mocks = parse_mock_rules(v, &mut issues);
                    }
                    "inspect_enabled" => match v {
                        ConfigValue::Bool(b) => cfg.inspect_enabled = *b,
                        _ => issues.push(ValidationIssue {
                            path: k.clone(),
                            message: "inspect_enabled must be bool".to_string(),
                        }),
                    },
                    "inspect_path" => match v {
                        ConfigValue::Str(s) if s.starts_with('/') => {
                            cfg.inspect_path =
                                s.trim_end_matches('/').to_string();
                            if cfg.inspect_path.is_empty() {
                                cfg.inspect_path = "/_d/inspect".to_string();
                            }
                        }
                        _ => issues.push(ValidationIssue {
                            path: k.clone(),
                            message: "inspect_path must start with `/`"
                                .to_string(),
                        }),
                    },
                    "inspect_max_entries" => match v {
                        ConfigValue::Int(n) if *n > 0 => {
                            cfg.inspect_max_entries = *n as usize;
                        }
                        _ => issues.push(ValidationIssue {
                            path: k.clone(),
                            message:
                                "inspect_max_entries must be positive integer"
                                    .to_string(),
                        }),
                    },
                    "live_reload" => match v {
                        ConfigValue::Bool(b) => cfg.live_reload.enabled = *b,
                        ConfigValue::Map(lrm) => {
                            cfg.live_reload.enabled = lrm
                                .get("enabled")
                                .and_then(cfg_bool)
                                .unwrap_or(true);
                            if let Some(ConfigValue::List(dirs)) =
                                lrm.get("watch_dirs")
                            {
                                cfg.live_reload.watch_dirs = dirs
                                    .iter()
                                    .filter_map(cfg_str)
                                    .map(PathBuf::from)
                                    .collect();
                            }
                            if let Some(ms) =
                                lrm.get("interval_ms").and_then(cfg_i64)
                                && ms > 0
                            {
                                cfg.live_reload.interval_ms = ms as u64;
                            }
                            if let Some(ws) =
                                lrm.get("ws_path").and_then(cfg_str)
                                && ws.starts_with('/')
                            {
                                cfg.live_reload.ws_path = ws.to_string();
                            }
                            if let Some(inj) =
                                lrm.get("inject_html").and_then(cfg_bool)
                            {
                                cfg.live_reload.inject_html = inj;
                            }
                        }
                        _ => issues.push(ValidationIssue {
                            path: k.clone(),
                            message: "live_reload must be bool or map"
                                .to_string(),
                        }),
                    },
                    _ => issues.push(ValidationIssue {
                        path: k.clone(),
                        message: format!("unknown ext-dev-kit option `{k}`"),
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
            message: "ext-dev-kit config must be Null or Map".to_string(),
        }])),
    }
}

impl Plugin for DevKitPlugin {
    fn name(&self) -> &str {
        "ext-dev-kit"
    }

    fn deps(&self) -> Deps {
        Deps::empty()
            .with::<RouterService>()
            .with_optional::<WsRegistryService>()
    }

    fn schema(&self) -> PluginSchema {
        PluginSchema::new(
            "Layer 3 development workflow kit: Mocks (HTTP+SSE), zero-CDN Live Reload, and white /_d/inspect UI",
        )
        .field(ConfigFieldSchema::new(
            "mocks",
            ConfigFieldType::List,
            "Declarative HTTP/SSE mock endpoints ([{id, method, path, status, headers, body, sse_chunks}])",
        ))
        .field(ConfigFieldSchema::new(
            "live_reload",
            ConfigFieldType::Map,
            "Live Reload watcher config ({enabled, watch_dirs, interval_ms, ws_path, inject_html})",
        ))
        .field(
            ConfigFieldSchema::new(
                "inspect_enabled",
                ConfigFieldType::Bool,
                "Enable white minimal HTTP/SSE inspector UI & API",
            )
            .with_default(ConfigValue::Bool(self.config.inspect_enabled)),
        )
        .field(
            ConfigFieldSchema::new(
                "inspect_path",
                ConfigFieldType::Str,
                "Base path for inspector UI and replay API",
            )
            .with_default(ConfigValue::Str(self.config.inspect_path.clone())),
        )
        .field(
            ConfigFieldSchema::new(
                "inspect_max_entries",
                ConfigFieldType::Int,
                "Ring buffer capacity for recorded requests",
            )
            .with_default(ConfigValue::Int(
                self.config.inspect_max_entries as i64,
            )),
        )
    }

    fn validate_config(
        &self,
        raw: &ConfigValue,
    ) -> Result<ConfigValue, FrameError> {
        let _ = resolve_dev_kit_config(&self.config, raw)?;
        Ok(raw.clone())
    }

    fn apply(
        &self,
        ctx: Context,
        config: ConfigValue,
    ) -> BoxFuture<'static, Result<(), FrameError>> {
        let base = self.config.clone();
        Box::pin(async move {
            let cfg = resolve_dev_kit_config(&base, &config)?;
            let router = ctx.use_service::<RouterService>()?;

            for rule in cfg.mocks {
                let _ = router.mount_route(
                    RouteSpec {
                        id: format!("dev-mock:{}", rule.id),
                        host: None,
                        matcher: RouteMatch::Exact(rule.path.clone()),
                        methods: Some(vec![rule.method.clone()]),
                        priority: 500,
                    },
                    Arc::new(MockHandler { rule }),
                )?;
            }

            if cfg.live_reload.enabled {
                let hub = LiveReloadHub::new();
                if let Some(ws_reg) = ctx.try_use_service::<WsRegistryService>()
                {
                    let handler: Arc<dyn WsHandler> =
                        Arc::new(LiveReloadWsHandler {
                            hub: Arc::clone(&hub),
                        });
                    let _ = ws_reg.mount_ws(
                        RouteSpec::new(
                            "dev-livereload-ws",
                            RouteMatch::Exact(cfg.live_reload.ws_path.clone()),
                        ),
                        handler,
                    )?;
                }
                if !cfg.live_reload.watch_dirs.is_empty() {
                    let _ = spawn_dir_watcher(
                        &ctx,
                        cfg.live_reload.watch_dirs.clone(),
                        cfg.live_reload.interval_ms,
                        hub,
                    )?;
                }
                if cfg.live_reload.inject_html {
                    let _ =
                        router.mount_filter(Arc::new(HtmlReloadInjector {
                            ws_path: cfg.live_reload.ws_path.clone(),
                            skip_prefix: cfg.inspect_path.clone(),
                        }))?;
                }
            }

            if cfg.inspect_enabled {
                let store = InspectStore::new(cfg.inspect_max_entries);
                let _ = router.mount_filter(Arc::new(InspectFilter {
                    inspect_prefix: cfg.inspect_path.clone(),
                    store: Arc::clone(&store),
                }))?;
                let _ = router.mount_route(
                    RouteSpec {
                        id: "dev-inspect-ui".to_string(),
                        host: None,
                        matcher: RouteMatch::Exact(cfg.inspect_path.clone()),
                        methods: Some(vec![Method::Get, Method::Head]),
                        priority: 950,
                    },
                    Arc::new(InspectUiHandler {
                        store: Arc::clone(&store),
                        base_path: cfg.inspect_path.clone(),
                    }),
                )?;
                let _ = router.mount_route(
                    RouteSpec {
                        id: "dev-inspect-events".to_string(),
                        host: None,
                        matcher: RouteMatch::Exact(format!(
                            "{}/api/events",
                            cfg.inspect_path
                        )),
                        methods: Some(vec![Method::Get, Method::Head]),
                        priority: 950,
                    },
                    Arc::new(InspectEventsJsonHandler {
                        store: Arc::clone(&store),
                    }),
                )?;
                let raw_router =
                    ctx.get::<RouterService>().ok_or_else(|| {
                        FrameError::new(
                            frame::ErrorKind::MissingDependency,
                            "RouterService missing",
                        )
                    })?;
                let _ = router.mount_route(
                    RouteSpec {
                        id: "dev-inspect-replay".to_string(),
                        host: None,
                        matcher: RouteMatch::Exact(format!(
                            "{}/api/replay",
                            cfg.inspect_path
                        )),
                        methods: Some(vec![Method::Post]),
                        priority: 950,
                    },
                    Arc::new(InspectReplayHandler {
                        store,
                        router: raw_router.as_ref().clone(),
                    }),
                )?;
            }

            Ok(())
        })
    }
}
