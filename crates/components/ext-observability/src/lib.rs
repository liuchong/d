//! Layer 3 zero-external-dependency observability extension:
//! structured access logging, OpenMetrics `/metrics`, and `/healthz` / `/readyz` probes.

use frame::{
    BoxFuture, ConfigFieldSchema, ConfigFieldType, ConfigValue, Context, Deps,
    FiberState, FrameError, Plugin, PluginSchema, ValidationIssue,
};
use http_api::{
    FilterOutcome, HttpFilter, HttpHandler, HttpRequest, HttpResponse, Method,
    RequestMeta, RouteMatch, RouteSpec, RouterService, StatusCode,
};
use net_api::TransportService;
use proxy_api::UpstreamPoolService;
use std::fmt::Write as _;
use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// Access log output encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessLogFormat {
    Off,
    Text,
    Json,
}

impl AccessLogFormat {
    /// Parse `off`, `text`, or `json`.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "off" => Some(Self::Off),
            "text" => Some(Self::Text),
            "json" => Some(Self::Json),
            _ => None,
        }
    }

    /// Return lowercase format token.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Text => "text",
            Self::Json => "json",
        }
    }
}

/// Configuration for [`ObservabilityPlugin`].
#[derive(Debug, Clone)]
pub struct ObservabilityConfig {
    pub log_format: AccessLogFormat,
    pub log_file: Option<PathBuf>,
    pub metrics_path: String,
    pub healthz_path: String,
    pub readyz_path: String,
}

impl Default for ObservabilityConfig {
    fn default() -> Self {
        Self {
            log_format: AccessLogFormat::Off,
            log_file: None,
            metrics_path: "/metrics".to_string(),
            healthz_path: "/healthz".to_string(),
            readyz_path: "/readyz".to_string(),
        }
    }
}

const BUCKET_BOUNDS_MS: [u64; 6] = [1, 5, 25, 100, 500, 2000];

struct MetricsState {
    req_2xx: AtomicU64,
    req_3xx: AtomicU64,
    req_4xx: AtomicU64,
    req_5xx: AtomicU64,
    req_other: AtomicU64,
    duration_count: AtomicU64,
    duration_sum_ms: AtomicU64,
    buckets: [AtomicU64; 6],
    log_file: Mutex<Option<std::fs::File>>,
    log_lines: Mutex<Vec<String>>,
}

impl MetricsState {
    fn new(log_file: Option<std::fs::File>) -> Self {
        Self {
            req_2xx: AtomicU64::new(0),
            req_3xx: AtomicU64::new(0),
            req_4xx: AtomicU64::new(0),
            req_5xx: AtomicU64::new(0),
            req_other: AtomicU64::new(0),
            duration_count: AtomicU64::new(0),
            duration_sum_ms: AtomicU64::new(0),
            buckets: std::array::from_fn(|_| AtomicU64::new(0)),
            log_file: Mutex::new(log_file),
            log_lines: Mutex::new(Vec::new()),
        }
    }

    fn record(&self, status: u16, elapsed_ms: u64, log_line: Option<String>) {
        match status / 100 {
            2 => {
                self.req_2xx.fetch_add(1, Ordering::Relaxed);
            }
            3 => {
                self.req_3xx.fetch_add(1, Ordering::Relaxed);
            }
            4 => {
                self.req_4xx.fetch_add(1, Ordering::Relaxed);
            }
            5 => {
                self.req_5xx.fetch_add(1, Ordering::Relaxed);
            }
            _ => {
                self.req_other.fetch_add(1, Ordering::Relaxed);
            }
        }
        self.duration_count.fetch_add(1, Ordering::Relaxed);
        self.duration_sum_ms
            .fetch_add(elapsed_ms, Ordering::Relaxed);
        for (idx, bound) in BUCKET_BOUNDS_MS.iter().enumerate() {
            if elapsed_ms <= *bound {
                self.buckets[idx].fetch_add(1, Ordering::Relaxed);
            }
        }
        if let Some(line) = log_line {
            let mut fguard =
                self.log_file.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(file) = fguard.as_mut() {
                let _ = writeln!(file, "{line}");
                let _ = file.flush();
            } else {
                eprintln!("{line}");
            }
            drop(fguard);
            let mut hist =
                self.log_lines.lock().unwrap_or_else(|e| e.into_inner());
            if hist.len() >= 128 {
                hist.remove(0);
            }
            hist.push(line);
        }
    }
}

fn json_escape(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len() + 2);
    out.push('"');
    for ch in raw.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

struct ObservabilityFilter {
    format: AccessLogFormat,
    state: Arc<MetricsState>,
}

impl HttpFilter for ObservabilityFilter {
    fn name(&self) -> &str {
        "ext-observability"
    }

    fn priority(&self) -> i32 {
        -1000
    }

    fn on_request(
        &self,
        req: HttpRequest,
    ) -> BoxFuture<'static, FilterOutcome> {
        Box::pin(async move { FilterOutcome::Continue(req) })
    }

    fn on_response(
        &self,
        req_meta: &RequestMeta,
        resp: HttpResponse,
    ) -> BoxFuture<'static, HttpResponse> {
        let elapsed = req_meta.started_at.elapsed();
        let elapsed_ms = elapsed.as_millis() as u64;
        let elapsed_us = elapsed.as_micros() as u64;
        let status = resp.status.as_u16();
        let len = resp.body.content_length().unwrap_or(0);
        let remote = req_meta
            .remote_addr
            .map_or_else(|| "-".to_string(), |a| a.ip().to_string());
        let host = req_meta.headers.get("host").unwrap_or("-");

        let log_line = match self.format {
            AccessLogFormat::Off => None,
            AccessLogFormat::Text => Some(format!(
                "{} {} {} status={} bytes={} elapsed_us={} remote={} host={}",
                req_meta.method.as_str(),
                req_meta.path,
                req_meta.version.as_str(),
                status,
                len,
                elapsed_us,
                remote,
                host
            )),
            AccessLogFormat::Json => Some(format!(
                "{{\"method\":{},\"path\":{},\"host\":{},\"remote\":{},\"status\":{},\"bytes\":{},\"elapsed_us\":{}}}",
                json_escape(req_meta.method.as_str()),
                json_escape(&req_meta.path),
                json_escape(host),
                json_escape(&remote),
                status,
                len,
                elapsed_us
            )),
        };

        self.state.record(status, elapsed_ms, log_line);
        Box::pin(async move { resp })
    }
}

struct HealthzHandler;

impl HttpHandler for HealthzHandler {
    fn call(&self, _req: HttpRequest) -> BoxFuture<'static, HttpResponse> {
        Box::pin(async move {
            HttpResponse::bytes(
                StatusCode::OK,
                "application/json",
                br#"{"status":"ok"}"#.to_vec(),
            )
        })
    }
}

struct ReadyzHandler {
    ctx: Context,
}

impl HttpHandler for ReadyzHandler {
    fn call(&self, _req: HttpRequest) -> BoxFuture<'static, HttpResponse> {
        let ctx = self.ctx.clone();
        Box::pin(async move {
            let snap = ctx.inspect();
            let has_failed_fiber = snap
                .fibers
                .iter()
                .any(|f| matches!(f.state, FiberState::Failed));
            let upstreams_ok = ctx
                .get::<UpstreamPoolService>()
                .is_none_or(|p| p.all_upstreams_have_healthy_backend());

            if !has_failed_fiber && upstreams_ok {
                HttpResponse::bytes(
                    StatusCode::OK,
                    "application/json",
                    br#"{"ready":true}"#.to_vec(),
                )
            } else {
                HttpResponse::bytes(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "application/json",
                    format!(
                        "{{\"ready\":false,\"failed_fibers\":{},\"upstreams_healthy\":{}}}",
                        has_failed_fiber, upstreams_ok
                    )
                    .into_bytes(),
                )
            }
        })
    }
}

struct MetricsHandler {
    ctx: Context,
    state: Arc<MetricsState>,
}

impl HttpHandler for MetricsHandler {
    fn call(&self, _req: HttpRequest) -> BoxFuture<'static, HttpResponse> {
        let ctx = self.ctx.clone();
        let state = Arc::clone(&self.state);
        Box::pin(async move {
            let mut body = String::with_capacity(1024);
            body.push_str(
                "# HELP d_http_requests_total Total HTTP responses by status class.\n\
                 # TYPE d_http_requests_total counter\n",
            );
            let _ = writeln!(
                body,
                "d_http_requests_total{{class=\"2xx\"}} {}",
                state.req_2xx.load(Ordering::Relaxed)
            );
            let _ = writeln!(
                body,
                "d_http_requests_total{{class=\"3xx\"}} {}",
                state.req_3xx.load(Ordering::Relaxed)
            );
            let _ = writeln!(
                body,
                "d_http_requests_total{{class=\"4xx\"}} {}",
                state.req_4xx.load(Ordering::Relaxed)
            );
            let _ = writeln!(
                body,
                "d_http_requests_total{{class=\"5xx\"}} {}",
                state.req_5xx.load(Ordering::Relaxed)
            );
            let _ = writeln!(
                body,
                "d_http_requests_total{{class=\"other\"}} {}",
                state.req_other.load(Ordering::Relaxed)
            );

            body.push_str(
                "# HELP d_http_request_duration_ms HTTP request duration in milliseconds.\n\
                 # TYPE d_http_request_duration_ms histogram\n",
            );
            for (idx, bound) in BUCKET_BOUNDS_MS.iter().enumerate() {
                let _ = writeln!(
                    body,
                    "d_http_request_duration_ms_bucket{{le=\"{bound}\"}} {}",
                    state.buckets[idx].load(Ordering::Relaxed)
                );
            }
            let total = state.duration_count.load(Ordering::Relaxed);
            let _ = writeln!(
                body,
                "d_http_request_duration_ms_bucket{{le=\"+Inf\"}} {total}"
            );
            let _ = writeln!(
                body,
                "d_http_request_duration_ms_sum {}",
                state.duration_sum_ms.load(Ordering::Relaxed)
            );
            let _ = writeln!(body, "d_http_request_duration_ms_count {total}");

            let snap = ctx.inspect();
            let active_fibers = snap
                .fibers
                .iter()
                .filter(|f| matches!(f.state, FiberState::Active))
                .count();
            let _ = writeln!(
                body,
                "# TYPE d_fibers_active gauge\nd_fibers_active {active_fibers}"
            );

            if let Some(transport) = ctx.get::<TransportService>() {
                let ts = transport.stats();
                let _ = writeln!(
                    body,
                    "# TYPE d_active_connections gauge\nd_active_connections {}",
                    ts.active_connections
                );
                let _ = writeln!(
                    body,
                    "# TYPE d_rejected_over_limit_total counter\nd_rejected_over_limit_total {}",
                    ts.rejected_over_limit
                );
            }

            if let Some(pool) = ctx.get::<UpstreamPoolService>() {
                body.push_str("# TYPE d_proxy_upstream_healthy gauge\n");
                for up in pool.snapshot() {
                    let healthy_cnt =
                        up.backends.iter().filter(|b| b.healthy).count();
                    let _ = writeln!(
                        body,
                        "d_proxy_upstream_healthy{{upstream=\"{}\"}} {healthy_cnt}",
                        up.id
                    );
                }
            }

            HttpResponse::bytes(
                StatusCode::OK,
                "text/plain; version=0.0.4; charset=utf-8",
                body.into_bytes(),
            )
        })
    }
}

/// Layer 3 plugin providing access logging, `/metrics`, `/healthz`, and `/readyz`.
#[derive(Debug, Clone, Default)]
pub struct ObservabilityPlugin {
    config: ObservabilityConfig,
}

impl ObservabilityPlugin {
    /// Create an observability plugin with custom defaults.
    pub fn new(config: ObservabilityConfig) -> Self {
        Self { config }
    }
}

fn resolve_obs_config(
    base: &ObservabilityConfig,
    raw: &ConfigValue,
) -> Result<ObservabilityConfig, FrameError> {
    let mut cfg = base.clone();
    match raw {
        ConfigValue::Null => Ok(cfg),
        ConfigValue::Map(map) => {
            let mut issues = Vec::new();
            for (k, v) in map {
                match k.as_str() {
                    "log_format" => match v {
                        ConfigValue::Str(s) => {
                            match AccessLogFormat::parse(s) {
                                Some(fmt) => cfg.log_format = fmt,
                                None => issues.push(ValidationIssue {
                                    path: k.clone(),
                                    message: format!(
                                        "unknown log_format `{s}`"
                                    ),
                                }),
                            }
                        }
                        _ => issues.push(ValidationIssue {
                            path: k.clone(),
                            message:
                                "log_format must be `off`, `text`, or `json`"
                                    .to_string(),
                        }),
                    },
                    "log_file" => match v {
                        ConfigValue::Null => cfg.log_file = None,
                        ConfigValue::Str(s) if !s.trim().is_empty() => {
                            cfg.log_file = Some(PathBuf::from(s));
                        }
                        _ => issues.push(ValidationIssue {
                            path: k.clone(),
                            message: "log_file must be a path string or null"
                                .to_string(),
                        }),
                    },
                    "metrics_path" | "healthz_path" | "readyz_path" => {
                        match v {
                            ConfigValue::Str(s) if s.starts_with('/') => {
                                match k.as_str() {
                                    "metrics_path" => {
                                        cfg.metrics_path = s.clone()
                                    }
                                    "healthz_path" => {
                                        cfg.healthz_path = s.clone()
                                    }
                                    "readyz_path" => {
                                        cfg.readyz_path = s.clone()
                                    }
                                    _ => {}
                                }
                            }
                            _ => issues.push(ValidationIssue {
                                path: k.clone(),
                                message: format!(
                                    "{k} must be a path starting with `/`"
                                ),
                            }),
                        }
                    }
                    _ => issues.push(ValidationIssue {
                        path: k.clone(),
                        message: format!(
                            "unknown ext-observability option `{k}`"
                        ),
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
            message: "ext-observability config must be Null or Map".to_string(),
        }])),
    }
}

impl Plugin for ObservabilityPlugin {
    fn name(&self) -> &str {
        "ext-observability"
    }

    fn deps(&self) -> Deps {
        Deps::empty().with::<RouterService>()
    }

    fn schema(&self) -> PluginSchema {
        PluginSchema::new(
            "Zero-dependency Layer 3 structured access logging, OpenMetrics /metrics, and /healthz /readyz probes",
        )
        .field(
            ConfigFieldSchema::new(
                "log_format",
                ConfigFieldType::Str,
                "Access log format: off, text, or json",
            )
            .with_default(ConfigValue::Str(
                self.config.log_format.as_str().to_string(),
            )),
        )
        .field(
            ConfigFieldSchema::new(
                "log_file",
                ConfigFieldType::Str,
                "Optional file path to append access logs (stderr when null)",
            )
            .nullable(),
        )
        .field(
            ConfigFieldSchema::new(
                "metrics_path",
                ConfigFieldType::Str,
                "HTTP path exposing OpenMetrics metrics",
            )
            .with_default(ConfigValue::Str(self.config.metrics_path.clone())),
        )
        .field(
            ConfigFieldSchema::new(
                "healthz_path",
                ConfigFieldType::Str,
                "HTTP liveness probe endpoint path",
            )
            .with_default(ConfigValue::Str(self.config.healthz_path.clone())),
        )
        .field(
            ConfigFieldSchema::new(
                "readyz_path",
                ConfigFieldType::Str,
                "HTTP readiness probe endpoint path",
            )
            .with_default(ConfigValue::Str(self.config.readyz_path.clone())),
        )
    }

    fn validate_config(
        &self,
        raw: &ConfigValue,
    ) -> Result<ConfigValue, FrameError> {
        let _ = resolve_obs_config(&self.config, raw)?;
        Ok(raw.clone())
    }

    fn apply(
        &self,
        ctx: Context,
        config: ConfigValue,
    ) -> BoxFuture<'static, Result<(), FrameError>> {
        let base = self.config.clone();
        Box::pin(async move {
            let cfg = resolve_obs_config(&base, &config)?;
            let router = ctx.use_service::<RouterService>()?;

            let log_file = if let Some(path) = &cfg.log_file {
                Some(
                    OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(path)
                        .map_err(|e| {
                            FrameError::new(
                                frame::ErrorKind::ActivationFailed,
                                format!(
                                    "failed to open access log `{}`: {e}",
                                    path.display()
                                ),
                            )
                        })?,
                )
            } else {
                None
            };

            let state = Arc::new(MetricsState::new(log_file));
            let _ = router.mount_filter(Arc::new(ObservabilityFilter {
                format: cfg.log_format,
                state: Arc::clone(&state),
            }))?;

            let _ = router.mount_route(
                RouteSpec {
                    id: "obs-healthz".to_string(),
                    host: None,
                    matcher: RouteMatch::Exact(cfg.healthz_path),
                    methods: Some(vec![Method::Get, Method::Head]),
                    priority: 900,
                },
                Arc::new(HealthzHandler),
            )?;
            let _ = router.mount_route(
                RouteSpec {
                    id: "obs-readyz".to_string(),
                    host: None,
                    matcher: RouteMatch::Exact(cfg.readyz_path),
                    methods: Some(vec![Method::Get, Method::Head]),
                    priority: 900,
                },
                Arc::new(ReadyzHandler { ctx: ctx.clone() }),
            )?;
            let _ = router.mount_route(
                RouteSpec {
                    id: "obs-metrics".to_string(),
                    host: None,
                    matcher: RouteMatch::Exact(cfg.metrics_path),
                    methods: Some(vec![Method::Get, Method::Head]),
                    priority: 900,
                },
                Arc::new(MetricsHandler { ctx, state }),
            )?;

            Ok(())
        })
    }
}
