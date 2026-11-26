//! Layer 3 L7/L4 reverse proxy plugin supporting Keep-Alive connection pooling,
//! Load Balancing, active/passive health checks, SSE passthrough, and WS tunnels.

pub mod health;
pub mod http_proxy;
pub mod lb;
pub mod pool;
pub mod tcp_proxy;
pub mod ws_tunnel;

use frame::{
    BoxFuture, ConfigFieldSchema, ConfigFieldType, ConfigValue, Context, Deps,
    ErrorKind, FrameError, Plugin, PluginSchema, ValidationIssue,
};
use health::spawn_health_probe;
use http_api::{HttpHandler, RouteMatch, RouteSpec, RouterService};
use http_proxy::HttpProxyHandler;
use lb::UpstreamState;
use net_api::{ConnectionHandler, ListenOptions, TransportService};
pub use proxy_api::{
    BackendConfig, BackendHealthSnapshot, HealthCheckConfig,
    HttpProxyRouteConfig, LoadBalanceStrategy, TcpProxyRouteConfig,
    UpstreamConfig, UpstreamHealthSnapshot, UpstreamPool, UpstreamPoolService,
};
use std::collections::{BTreeMap, BTreeSet};
use std::net::SocketAddr;
use std::sync::Arc;
use tcp_proxy::TcpProxyHandler;

/// Top-level programmatic or declarative configuration for [`ProxyPlugin`].
#[derive(Debug, Clone, Default)]
pub struct ProxyConfig {
    pub upstreams: Vec<UpstreamConfig>,
    pub http_routes: Vec<HttpProxyRouteConfig>,
    pub tcp_routes: Vec<TcpProxyRouteConfig>,
}

struct RegistryUpstreamPool {
    upstreams: Vec<Arc<UpstreamState>>,
}

impl UpstreamPool for RegistryUpstreamPool {
    fn snapshot(&self) -> Vec<UpstreamHealthSnapshot> {
        self.upstreams.iter().map(|u| u.snapshot()).collect()
    }
}

/// Layer 3 reverse proxy plugin providing [`UpstreamPoolService`] and mounting
/// L7 HTTP/SSE/WebSocket & L4 TCP proxy routes.
#[derive(Debug, Clone, Default)]
pub struct ProxyPlugin {
    config: ProxyConfig,
}

impl ProxyPlugin {
    /// Construct a reverse proxy plugin with initial config defaults.
    pub fn new(config: ProxyConfig) -> Self {
        Self { config }
    }
}

fn as_pos_u64(
    val: &ConfigValue,
    path: &str,
    issues: &mut Vec<ValidationIssue>,
) -> Option<u64> {
    match val {
        ConfigValue::Int(n) if *n > 0 => Some(*n as u64),
        _ => {
            issues.push(ValidationIssue {
                path: path.to_string(),
                message: "must be a positive integer".to_string(),
            });
            None
        }
    }
}

fn as_nonneg_u64(
    val: &ConfigValue,
    path: &str,
    issues: &mut Vec<ValidationIssue>,
) -> Option<u64> {
    match val {
        ConfigValue::Int(n) if *n >= 0 => Some(*n as u64),
        _ => {
            issues.push(ValidationIssue {
                path: path.to_string(),
                message: "must be a non-negative integer".to_string(),
            });
            None
        }
    }
}

fn parse_backend(
    raw: &ConfigValue,
    prefix: &str,
    issues: &mut Vec<ValidationIssue>,
) -> Option<BackendConfig> {
    let ConfigValue::Map(map) = raw else {
        issues.push(ValidationIssue {
            path: prefix.to_string(),
            message: "backend entry must be a map".to_string(),
        });
        return None;
    };
    let mut addr = None;
    let mut weight = 1u32;
    let mut tls = false;
    let mut sni = None;
    let mut ca_cert_pem = None;
    let mut insecure_skip_verify = false;

    for (k, v) in map {
        match k.as_str() {
            "addr" => match v {
                ConfigValue::Str(s) => match s.parse::<SocketAddr>() {
                    Ok(a) => addr = Some(a),
                    Err(e) => issues.push(ValidationIssue {
                        path: format!("{prefix}.addr"),
                        message: format!("invalid backend addr `{s}`: {e}"),
                    }),
                },
                _ => issues.push(ValidationIssue {
                    path: format!("{prefix}.addr"),
                    message: "addr must be a socket address string".to_string(),
                }),
            },
            "weight" => {
                if let Some(w) =
                    as_pos_u64(v, &format!("{prefix}.weight"), issues)
                {
                    weight = w.min(1000) as u32;
                }
            }
            "tls" => match v {
                ConfigValue::Bool(b) => tls = *b,
                _ => issues.push(ValidationIssue {
                    path: format!("{prefix}.tls"),
                    message: "tls must be a boolean".to_string(),
                }),
            },
            "sni" => match v {
                ConfigValue::Null => sni = None,
                ConfigValue::Str(s) => sni = Some(s.clone()),
                _ => issues.push(ValidationIssue {
                    path: format!("{prefix}.sni"),
                    message: "sni must be a string or null".to_string(),
                }),
            },
            "ca_cert_pem" => match v {
                ConfigValue::Null => ca_cert_pem = None,
                ConfigValue::Str(s) => ca_cert_pem = Some(s.clone()),
                _ => issues.push(ValidationIssue {
                    path: format!("{prefix}.ca_cert_pem"),
                    message: "ca_cert_pem must be a string or null".to_string(),
                }),
            },
            "insecure_skip_verify" => match v {
                ConfigValue::Bool(b) => insecure_skip_verify = *b,
                _ => issues.push(ValidationIssue {
                    path: format!("{prefix}.insecure_skip_verify"),
                    message: "insecure_skip_verify must be a boolean"
                        .to_string(),
                }),
            },
            _ => issues.push(ValidationIssue {
                path: format!("{prefix}.{k}"),
                message: format!("unknown backend option `{k}`"),
            }),
        }
    }

    let Some(addr) = addr else {
        issues.push(ValidationIssue {
            path: format!("{prefix}.addr"),
            message: "backend addr is required".to_string(),
        });
        return None;
    };
    Some(BackendConfig {
        addr,
        weight,
        tls,
        sni,
        ca_cert_pem,
        insecure_skip_verify,
    })
}

fn parse_health_check(
    raw: &ConfigValue,
    prefix: &str,
    issues: &mut Vec<ValidationIssue>,
) -> Option<HealthCheckConfig> {
    if matches!(raw, ConfigValue::Null) {
        return None;
    }
    let ConfigValue::Map(map) = raw else {
        issues.push(ValidationIssue {
            path: prefix.to_string(),
            message: "active_health_check must be a map or null".to_string(),
        });
        return None;
    };
    let mut hc = HealthCheckConfig::default();
    for (k, v) in map {
        match k.as_str() {
            "interval_ms" => {
                if let Some(n) =
                    as_pos_u64(v, &format!("{prefix}.interval_ms"), issues)
                {
                    hc.interval_ms = n;
                }
            }
            "timeout_ms" => {
                if let Some(n) =
                    as_pos_u64(v, &format!("{prefix}.timeout_ms"), issues)
                {
                    hc.timeout_ms = n;
                }
            }
            "path" => match v {
                ConfigValue::Null => hc.path = None,
                ConfigValue::Str(s) => hc.path = Some(s.clone()),
                _ => issues.push(ValidationIssue {
                    path: format!("{prefix}.path"),
                    message: "path must be a string or null".to_string(),
                }),
            },
            "unhealthy_threshold" => {
                if let Some(n) = as_pos_u64(
                    v,
                    &format!("{prefix}.unhealthy_threshold"),
                    issues,
                ) {
                    hc.unhealthy_threshold = n as u32;
                }
            }
            "healthy_threshold" => {
                if let Some(n) = as_pos_u64(
                    v,
                    &format!("{prefix}.healthy_threshold"),
                    issues,
                ) {
                    hc.healthy_threshold = n as u32;
                }
            }
            _ => issues.push(ValidationIssue {
                path: format!("{prefix}.{k}"),
                message: format!("unknown health check option `{k}`"),
            }),
        }
    }
    Some(hc)
}

fn resolve_proxy_config(
    base: &ProxyConfig,
    raw: &ConfigValue,
) -> Result<ProxyConfig, FrameError> {
    let mut cfg = base.clone();
    let mut issues = Vec::new();
    match raw {
        ConfigValue::Null => {}
        ConfigValue::Map(map) => {
            for (k, v) in map {
                match k.as_str() {
                    "upstreams" => {
                        let ConfigValue::List(list) = v else {
                            issues.push(ValidationIssue {
                                path: "upstreams".to_string(),
                                message: "upstreams must be a list".to_string(),
                            });
                            continue;
                        };
                        cfg.upstreams.clear();
                        for (idx, item) in list.iter().enumerate() {
                            let prefix = format!("upstreams[{idx}]");
                            let ConfigValue::Map(umap) = item else {
                                issues.push(ValidationIssue {
                                    path: prefix,
                                    message: "upstream entry must be a map"
                                        .to_string(),
                                });
                                continue;
                            };
                            let mut id = None;
                            let mut up = UpstreamConfig::new("", Vec::new());
                            for (uk, uv) in umap {
                                match uk.as_str() {
                                    "id" => match uv {
                                        ConfigValue::Str(s)
                                            if !s.trim().is_empty() =>
                                        {
                                            id = Some(s.clone());
                                        }
                                        _ => issues.push(ValidationIssue {
                                            path: format!("{prefix}.id"),
                                            message: "upstream id must be non-empty string".to_string(),
                                        }),
                                    },
                                    "strategy" => match uv {
                                        ConfigValue::Str(s) => {
                                            match LoadBalanceStrategy::parse(s) {
                                                Some(st) => up.strategy = st,
                                                None => issues.push(ValidationIssue {
                                                    path: format!("{prefix}.strategy"),
                                                    message: format!("unknown LB strategy `{s}`"),
                                                }),
                                            }
                                        }
                                        _ => issues.push(ValidationIssue {
                                            path: format!("{prefix}.strategy"),
                                            message: "strategy must be a string".to_string(),
                                        }),
                                    },
                                    "backends" => {
                                        let ConfigValue::List(bl) = uv else {
                                            issues.push(ValidationIssue {
                                                path: format!("{prefix}.backends"),
                                                message: "backends must be a list".to_string(),
                                            });
                                            continue;
                                        };
                                        for (bidx, bv) in bl.iter().enumerate() {
                                            if let Some(b) = parse_backend(
                                                bv,
                                                &format!("{prefix}.backends[{bidx}]"),
                                                &mut issues,
                                            ) {
                                                up.backends.push(b);
                                            }
                                        }
                                    }
                                    "max_idle_per_backend" => {
                                        if let Some(n) = as_nonneg_u64(
                                            uv,
                                            &format!("{prefix}.max_idle_per_backend"),
                                            &mut issues,
                                        ) {
                                            up.max_idle_per_backend = n as usize;
                                        }
                                    }
                                    "connect_timeout_ms" => {
                                        if let Some(n) = as_pos_u64(
                                            uv,
                                            &format!("{prefix}.connect_timeout_ms"),
                                            &mut issues,
                                        ) {
                                            up.connect_timeout_ms = n;
                                        }
                                    }
                                    "response_header_timeout_ms" => {
                                        if let Some(n) = as_pos_u64(
                                            uv,
                                            &format!("{prefix}.response_header_timeout_ms"),
                                            &mut issues,
                                        ) {
                                            up.response_header_timeout_ms = n;
                                        }
                                    }
                                    "active_health_check" => {
                                        up.active_health_check = parse_health_check(
                                            uv,
                                            &format!("{prefix}.active_health_check"),
                                            &mut issues,
                                        );
                                    }
                                    "passive_eject_threshold" => {
                                        if let Some(n) = as_nonneg_u64(
                                            uv,
                                            &format!("{prefix}.passive_eject_threshold"),
                                            &mut issues,
                                        ) {
                                            up.passive_eject_threshold = n as u32;
                                        }
                                    }
                                    "passive_eject_ms" => {
                                        if let Some(n) = as_nonneg_u64(
                                            uv,
                                            &format!("{prefix}.passive_eject_ms"),
                                            &mut issues,
                                        ) {
                                            up.passive_eject_ms = n;
                                        }
                                    }
                                    _ => issues.push(ValidationIssue {
                                        path: format!("{prefix}.{uk}"),
                                        message: format!("unknown upstream field `{uk}`"),
                                    }),
                                }
                            }
                            if let Some(uid) = id {
                                up.id = uid;
                                cfg.upstreams.push(up);
                            } else {
                                issues.push(ValidationIssue {
                                    path: format!("{prefix}.id"),
                                    message: "upstream id is required"
                                        .to_string(),
                                });
                            }
                        }
                    }
                    "http_routes" => {
                        let ConfigValue::List(list) = v else {
                            issues.push(ValidationIssue {
                                path: "http_routes".to_string(),
                                message: "http_routes must be a list"
                                    .to_string(),
                            });
                            continue;
                        };
                        cfg.http_routes.clear();
                        for (idx, item) in list.iter().enumerate() {
                            let prefix = format!("http_routes[{idx}]");
                            let ConfigValue::Map(rmap) = item else {
                                issues.push(ValidationIssue {
                                    path: prefix,
                                    message: "http route must be a map"
                                        .to_string(),
                                });
                                continue;
                            };
                            let mut route = HttpProxyRouteConfig {
                                id: String::new(),
                                host: None,
                                match_prefix: "/".to_string(),
                                strip_prefix: None,
                                preserve_host: false,
                                upstream: String::new(),
                                websocket: true,
                                retry_idempotent: true,
                                extra_headers: Vec::new(),
                            };
                            for (rk, rv) in rmap {
                                match rk.as_str() {
                                    "id" => match rv {
                                        ConfigValue::Str(s) if !s.trim().is_empty() => {
                                            route.id = s.clone();
                                        }
                                        _ => issues.push(ValidationIssue {
                                            path: format!("{prefix}.id"),
                                            message: "route id must be non-empty string".to_string(),
                                        }),
                                    },
                                    "host" => match rv {
                                        ConfigValue::Null => route.host = None,
                                        ConfigValue::Str(s) => route.host = Some(s.clone()),
                                        _ => issues.push(ValidationIssue {
                                            path: format!("{prefix}.host"),
                                            message: "host must be string or null".to_string(),
                                        }),
                                    },
                                    "match_prefix" => match rv {
                                        ConfigValue::Str(s) if s.starts_with('/') => {
                                            route.match_prefix = s.clone();
                                        }
                                        _ => issues.push(ValidationIssue {
                                            path: format!("{prefix}.match_prefix"),
                                            message: "match_prefix must start with `/`".to_string(),
                                        }),
                                    },
                                    "strip_prefix" => match rv {
                                        ConfigValue::Null => route.strip_prefix = None,
                                        ConfigValue::Str(s) => route.strip_prefix = Some(s.clone()),
                                        _ => issues.push(ValidationIssue {
                                            path: format!("{prefix}.strip_prefix"),
                                            message: "strip_prefix must be string or null".to_string(),
                                        }),
                                    },
                                    "preserve_host" => match rv {
                                        ConfigValue::Bool(b) => route.preserve_host = *b,
                                        _ => issues.push(ValidationIssue {
                                            path: format!("{prefix}.preserve_host"),
                                            message: "preserve_host must be boolean".to_string(),
                                        }),
                                    },
                                    "upstream" => match rv {
                                        ConfigValue::Str(s) if !s.trim().is_empty() => {
                                            route.upstream = s.clone();
                                        }
                                        _ => issues.push(ValidationIssue {
                                            path: format!("{prefix}.upstream"),
                                            message: "upstream must be non-empty string".to_string(),
                                        }),
                                    },
                                    "websocket" => match rv {
                                        ConfigValue::Bool(b) => route.websocket = *b,
                                        _ => issues.push(ValidationIssue {
                                            path: format!("{prefix}.websocket"),
                                            message: "websocket must be boolean".to_string(),
                                        }),
                                    },
                                    "retry_idempotent" => match rv {
                                        ConfigValue::Bool(b) => route.retry_idempotent = *b,
                                        _ => issues.push(ValidationIssue {
                                            path: format!("{prefix}.retry_idempotent"),
                                            message: "retry_idempotent must be boolean".to_string(),
                                        }),
                                    },
                                    "extra_headers" => match rv {
                                        ConfigValue::Map(hm) => {
                                            for (hk, hv) in hm {
                                                if let ConfigValue::Str(s) = hv {
                                                    route.extra_headers.push((hk.clone(), s.clone()));
                                                } else {
                                                    issues.push(ValidationIssue {
                                                        path: format!("{prefix}.extra_headers.{hk}"),
                                                        message: "header value must be string".to_string(),
                                                    });
                                                }
                                            }
                                        }
                                        _ => issues.push(ValidationIssue {
                                            path: format!("{prefix}.extra_headers"),
                                            message: "extra_headers must be map".to_string(),
                                        }),
                                    },
                                    _ => issues.push(ValidationIssue {
                                        path: format!("{prefix}.{rk}"),
                                        message: format!("unknown http_route field `{rk}`"),
                                    }),
                                }
                            }
                            if route.id.is_empty() || route.upstream.is_empty()
                            {
                                issues.push(ValidationIssue {
                                    path: prefix,
                                    message:
                                        "http_route requires id and upstream"
                                            .to_string(),
                                });
                            } else {
                                cfg.http_routes.push(route);
                            }
                        }
                    }
                    "tcp_routes" => {
                        let ConfigValue::List(list) = v else {
                            issues.push(ValidationIssue {
                                path: "tcp_routes".to_string(),
                                message: "tcp_routes must be a list"
                                    .to_string(),
                            });
                            continue;
                        };
                        cfg.tcp_routes.clear();
                        for (idx, item) in list.iter().enumerate() {
                            let prefix = format!("tcp_routes[{idx}]");
                            let ConfigValue::Map(tmap) = item else {
                                issues.push(ValidationIssue {
                                    path: prefix,
                                    message: "tcp route must be a map"
                                        .to_string(),
                                });
                                continue;
                            };
                            let mut id = None;
                            let mut listen_addr = None;
                            let mut tls_listener_id = None;
                            let mut max_connections = None;
                            let mut upstream = None;
                            for (tk, tv) in tmap {
                                match tk.as_str() {
                                    "id" => match tv {
                                        ConfigValue::Str(s) if !s.trim().is_empty() => {
                                            id = Some(s.clone());
                                        }
                                        _ => issues.push(ValidationIssue {
                                            path: format!("{prefix}.id"),
                                            message: "tcp route id must be non-empty string".to_string(),
                                        }),
                                    },
                                    "listen_addr" => match tv {
                                        ConfigValue::Str(s) => match s.parse::<SocketAddr>() {
                                            Ok(a) => listen_addr = Some(a),
                                            Err(e) => issues.push(ValidationIssue {
                                                path: format!("{prefix}.listen_addr"),
                                                message: format!("invalid listen_addr `{s}`: {e}"),
                                            }),
                                        },
                                        _ => issues.push(ValidationIssue {
                                            path: format!("{prefix}.listen_addr"),
                                            message: "listen_addr must be socket address string".to_string(),
                                        }),
                                    },
                                    "tls_listener_id" => match tv {
                                        ConfigValue::Null => tls_listener_id = None,
                                        ConfigValue::Str(s) => tls_listener_id = Some(s.clone()),
                                        _ => issues.push(ValidationIssue {
                                            path: format!("{prefix}.tls_listener_id"),
                                            message: "tls_listener_id must be string or null".to_string(),
                                        }),
                                    },
                                    "max_connections" => match tv {
                                        ConfigValue::Null => max_connections = None,
                                        ConfigValue::Int(n) if *n > 0 => {
                                            max_connections = Some(*n as usize);
                                        }
                                        _ => issues.push(ValidationIssue {
                                            path: format!("{prefix}.max_connections"),
                                            message: "max_connections must be positive int or null".to_string(),
                                        }),
                                    },
                                    "upstream" => match tv {
                                        ConfigValue::Str(s) if !s.trim().is_empty() => {
                                            upstream = Some(s.clone());
                                        }
                                        _ => issues.push(ValidationIssue {
                                            path: format!("{prefix}.upstream"),
                                            message: "upstream must be non-empty string".to_string(),
                                        }),
                                    },
                                    _ => issues.push(ValidationIssue {
                                        path: format!("{prefix}.{tk}"),
                                        message: format!("unknown tcp_route field `{tk}`"),
                                    }),
                                }
                            }
                            if let (
                                Some(id),
                                Some(listen_addr),
                                Some(upstream),
                            ) = (id, listen_addr, upstream)
                            {
                                cfg.tcp_routes.push(TcpProxyRouteConfig {
                                    id,
                                    listen_addr,
                                    tls_listener_id,
                                    max_connections,
                                    upstream,
                                });
                            } else {
                                issues.push(ValidationIssue {
                                    path: prefix,
                                    message: "tcp_route requires id, listen_addr, and upstream".to_string(),
                                });
                            }
                        }
                    }
                    _ => issues.push(ValidationIssue {
                        path: k.clone(),
                        message: format!("unknown ext-proxy option `{k}`"),
                    }),
                }
            }
        }
        _ => {
            return Err(FrameError::validation(vec![ValidationIssue {
                path: "".to_string(),
                message: "ext-proxy config must be Null or Map".to_string(),
            }]));
        }
    }

    let mut seen_upstreams = BTreeSet::new();
    for (idx, u) in cfg.upstreams.iter().enumerate() {
        if !seen_upstreams.insert(u.id.clone()) {
            issues.push(ValidationIssue {
                path: format!("upstreams[{idx}].id"),
                message: format!("duplicate upstream `{}`", u.id),
            });
        }
    }
    for (idx, r) in cfg.http_routes.iter().enumerate() {
        if !seen_upstreams.contains(&r.upstream) {
            issues.push(ValidationIssue {
                path: format!("http_routes[{idx}].upstream"),
                message: format!("unknown upstream `{}`", r.upstream),
            });
        }
    }
    for (idx, r) in cfg.tcp_routes.iter().enumerate() {
        if !seen_upstreams.contains(&r.upstream) {
            issues.push(ValidationIssue {
                path: format!("tcp_routes[{idx}].upstream"),
                message: format!("unknown upstream `{}`", r.upstream),
            });
        }
    }

    if issues.is_empty() {
        Ok(cfg)
    } else {
        Err(FrameError::validation(issues))
    }
}

impl Plugin for ProxyPlugin {
    fn name(&self) -> &str {
        "ext-proxy"
    }

    fn deps(&self) -> Deps {
        Deps::empty()
            .with::<RouterService>()
            .with_optional::<TransportService>()
    }

    fn schema(&self) -> PluginSchema {
        PluginSchema::new(
            "Layer 3 generic L7/L4 reverse proxy with keep-alive pooling, load balancing, health checks, SSE passthrough, and WebSocket tunneling",
        )
        .field(ConfigFieldSchema::new(
            "upstreams",
            ConfigFieldType::List,
            "List of upstream pools ({id, strategy, backends, max_idle_per_backend, connect_timeout_ms, response_header_timeout_ms, active_health_check, passive_eject_threshold, passive_eject_ms})",
        ))
        .field(ConfigFieldSchema::new(
            "http_routes",
            ConfigFieldType::List,
            "List of L7 HTTP/SSE/WebSocket proxy routes ({id, host, match_prefix, strip_prefix, preserve_host, upstream, websocket, retry_idempotent, extra_headers})",
        ))
        .field(ConfigFieldSchema::new(
            "tcp_routes",
            ConfigFieldType::List,
            "List of L4 TCP proxy listeners ({id, listen_addr, tls_listener_id, max_connections, upstream})",
        ))
    }

    fn validate_config(
        &self,
        raw: &ConfigValue,
    ) -> Result<ConfigValue, FrameError> {
        let _ = resolve_proxy_config(&self.config, raw)?;
        Ok(raw.clone())
    }

    fn apply(
        &self,
        ctx: Context,
        config: ConfigValue,
    ) -> BoxFuture<'static, Result<(), FrameError>> {
        let base = self.config.clone();
        Box::pin(async move {
            let cfg = resolve_proxy_config(&base, &config)?;
            let router = ctx.use_service::<RouterService>()?;

            let mut upstream_map: BTreeMap<String, Arc<UpstreamState>> =
                BTreeMap::new();
            let mut all_upstreams = Vec::with_capacity(cfg.upstreams.len());

            for ucfg in cfg.upstreams {
                let hc = ucfg.active_health_check.clone();
                let st = UpstreamState::new(ucfg.clone());
                if let Some(hc_cfg) = hc {
                    let _ = spawn_health_probe(&ctx, Arc::clone(&st), hc_cfg)?;
                }
                upstream_map.insert(ucfg.id.clone(), Arc::clone(&st));
                all_upstreams.push(st);
            }

            let pool_svc: Arc<UpstreamPoolService> = Arc::new(
                UpstreamPoolService::new(Arc::new(RegistryUpstreamPool {
                    upstreams: all_upstreams,
                })),
            );
            ctx.provide(pool_svc).await?;

            for route in cfg.http_routes {
                let up = upstream_map
                    .get(&route.upstream)
                    .cloned()
                    .ok_or_else(|| {
                        FrameError::new(
                            ErrorKind::ValidationFailed,
                            format!("missing upstream `{}`", route.upstream),
                        )
                    })?;
                let handler: Arc<dyn HttpHandler> =
                    Arc::new(HttpProxyHandler {
                        route: route.clone(),
                        upstream: up,
                        ctx: ctx.clone(),
                    });
                let _ = router.mount_route(
                    RouteSpec {
                        id: format!("proxy-http:{}", route.id),
                        host: route.host,
                        matcher: RouteMatch::Prefix(route.match_prefix),
                        methods: None,
                        priority: 10,
                    },
                    handler,
                )?;
            }

            if !cfg.tcp_routes.is_empty() {
                let transport =
                    ctx.try_use_service::<TransportService>().ok_or_else(|| {
                        FrameError::new(
                            ErrorKind::MissingDependency,
                            "TransportService is required when tcp_routes are configured",
                        )
                    })?;
                for route in cfg.tcp_routes {
                    let up = upstream_map
                        .get(&route.upstream)
                        .cloned()
                        .ok_or_else(|| {
                            FrameError::new(
                                ErrorKind::ValidationFailed,
                                format!(
                                    "missing upstream `{}`",
                                    route.upstream
                                ),
                            )
                        })?;
                    let handler: Arc<dyn ConnectionHandler> =
                        Arc::new(TcpProxyHandler {
                            upstream: up,
                            ctx: ctx.clone(),
                        });
                    let _ = transport
                        .listen_with_options(
                            ListenOptions {
                                addr: route.listen_addr,
                                max_connections: route.max_connections,
                                tls_listener_id: route.tls_listener_id,
                            },
                            handler,
                        )
                        .await?;
                }
            }

            Ok(())
        })
    }
}
