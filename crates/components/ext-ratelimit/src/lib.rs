//! Zero-dependency Layer 3 token-bucket rate limiting [`HttpFilter`].

use frame::{
    BoxFuture, ConfigFieldSchema, ConfigFieldType, ConfigValue, Context, Deps,
    FrameError, Plugin, PluginSchema, ValidationIssue,
};
use http_api::{
    FilterOutcome, HttpFilter, HttpRequest, HttpResponse, RequestMeta,
    RouterService, StatusCode,
};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Rate limit rule scoping a path prefix and token bucket parameters.
#[derive(Debug, Clone, PartialEq)]
pub struct RateLimitRule {
    pub prefix: String,
    pub rps: f64,
    pub burst: f64,
    pub trust_x_forwarded_for: bool,
}

/// Top-level configuration for [`RateLimitPlugin`].
#[derive(Debug, Clone)]
pub struct RateLimitConfig {
    pub rules: Vec<RateLimitRule>,
    pub max_entries: usize,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            rules: vec![RateLimitRule {
                prefix: "/".to_string(),
                rps: 100.0,
                burst: 200.0,
                trust_x_forwarded_for: false,
            }],
            max_entries: 4096,
        }
    }
}

struct Bucket {
    tokens: f64,
    last: Instant,
}

impl Bucket {
    fn new(burst: f64, now: Instant) -> Self {
        Self {
            tokens: burst,
            last: now,
        }
    }

    fn take(&mut self, rps: f64, burst: f64, now: Instant) -> Result<u64, u64> {
        let elapsed = now.duration_since(self.last).as_secs_f64();
        self.last = now;
        self.tokens = (self.tokens + elapsed * rps).min(burst);
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            Ok(self.tokens.floor() as u64)
        } else {
            let deficit = 1.0 - self.tokens;
            let retry_secs = (deficit / rps.max(0.001)).ceil().max(1.0) as u64;
            Err(retry_secs)
        }
    }
}

struct RateLimitFilter {
    rules: Vec<RateLimitRule>,
    max_entries: usize,
    buckets: Mutex<HashMap<(usize, String), Bucket>>,
}

impl RateLimitFilter {
    fn client_key(req: &HttpRequest, trust_xff: bool) -> String {
        if trust_xff
            && let Some(xff) = req.headers.get("x-forwarded-for")
            && let Some(first) = xff.split(',').next()
        {
            let trimmed = first.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
        req.remote_addr
            .map_or_else(|| "local".to_string(), |a| a.ip().to_string())
    }
}

impl HttpFilter for RateLimitFilter {
    fn name(&self) -> &str {
        "ext-ratelimit"
    }

    fn priority(&self) -> i32 {
        -500
    }

    fn on_request(
        &self,
        req: HttpRequest,
    ) -> BoxFuture<'static, FilterOutcome> {
        let matched = self
            .rules
            .iter()
            .enumerate()
            .filter(|(_, r)| req.path.starts_with(&r.prefix))
            .max_by_key(|(_, r)| r.prefix.len());

        let Some((rule_idx, rule)) = matched else {
            return Box::pin(async move { FilterOutcome::Continue(req) });
        };

        let key =
            (rule_idx, Self::client_key(&req, rule.trust_x_forwarded_for));
        let now = Instant::now();
        let outcome = {
            let mut guard =
                self.buckets.lock().unwrap_or_else(|e| e.into_inner());
            if guard.len() >= self.max_entries && !guard.contains_key(&key) {
                guard.retain(|_, b| {
                    now.duration_since(b.last) < Duration::from_secs(120)
                });
                if guard.len() >= self.max_entries {
                    guard.clear();
                }
            }
            let bucket = guard
                .entry(key)
                .or_insert_with(|| Bucket::new(rule.burst, now));
            bucket.take(rule.rps, rule.burst, now)
        };

        Box::pin(async move {
            match outcome {
                Ok(_remaining) => FilterOutcome::Continue(req),
                Err(retry_after) => FilterOutcome::Respond(
                    HttpResponse::text(
                        StatusCode::TOO_MANY_REQUESTS,
                        "Too Many Requests",
                    )
                    .with_header("Retry-After", retry_after.to_string())
                    .with_header("X-RateLimit-Remaining", "0"),
                ),
            }
        })
    }

    fn on_response(
        &self,
        _req_meta: &RequestMeta,
        resp: HttpResponse,
    ) -> BoxFuture<'static, HttpResponse> {
        Box::pin(async move { resp })
    }
}

/// Layer 3 plugin applying token-bucket rate limiting across routes.
#[derive(Debug, Clone, Default)]
pub struct RateLimitPlugin {
    config: RateLimitConfig,
}

impl RateLimitPlugin {
    /// Construct a rate-limit plugin with default configuration.
    pub fn new(config: RateLimitConfig) -> Self {
        Self { config }
    }
}

fn as_pos_f64(
    v: &ConfigValue,
    path: &str,
    issues: &mut Vec<ValidationIssue>,
) -> Option<f64> {
    match v {
        ConfigValue::Int(n) if *n > 0 => Some(*n as f64),
        _ => {
            issues.push(ValidationIssue {
                path: path.to_string(),
                message: "must be a positive integer".to_string(),
            });
            None
        }
    }
}

fn resolve_ratelimit_config(
    base: &RateLimitConfig,
    raw: &ConfigValue,
) -> Result<RateLimitConfig, FrameError> {
    let mut cfg = base.clone();
    match raw {
        ConfigValue::Null => Ok(cfg),
        ConfigValue::Map(map) => {
            let mut issues = Vec::new();
            let mut global_rps = None;
            let mut global_burst = None;

            for (k, v) in map {
                match k.as_str() {
                    "rps" => {
                        global_rps = as_pos_f64(v, "rps", &mut issues);
                    }
                    "burst" => {
                        global_burst = as_pos_f64(v, "burst", &mut issues);
                    }
                    "max_entries" => match v {
                        ConfigValue::Int(n) if *n > 0 => {
                            cfg.max_entries = *n as usize;
                        }
                        _ => issues.push(ValidationIssue {
                            path: k.clone(),
                            message: "max_entries must be positive integer"
                                .to_string(),
                        }),
                    },
                    "rules" => {
                        let ConfigValue::List(arr) = v else {
                            issues.push(ValidationIssue {
                                path: "rules".to_string(),
                                message: "rules must be a list".to_string(),
                            });
                            continue;
                        };
                        cfg.rules.clear();
                        for (idx, item) in arr.iter().enumerate() {
                            let prefix = format!("rules[{idx}]");
                            let ConfigValue::Map(rmap) = item else {
                                issues.push(ValidationIssue {
                                    path: prefix,
                                    message: "rule must be a map".to_string(),
                                });
                                continue;
                            };
                            let mut rule_prefix = "/".to_string();
                            let mut rps = 100.0;
                            let mut burst = 200.0;
                            let mut trust_xff = false;
                            for (rk, rv) in rmap {
                                match rk.as_str() {
                                    "prefix" => match rv {
                                        ConfigValue::Str(s)
                                            if s.starts_with('/') =>
                                        {
                                            rule_prefix = s.clone();
                                        }
                                        _ => issues.push(ValidationIssue {
                                            path: format!("{prefix}.prefix"),
                                            message:
                                                "prefix must start with `/`"
                                                    .to_string(),
                                        }),
                                    },
                                    "rps" => {
                                        if let Some(n) = as_pos_f64(
                                            rv,
                                            &format!("{prefix}.rps"),
                                            &mut issues,
                                        ) {
                                            rps = n;
                                        }
                                    }
                                    "burst" => {
                                        if let Some(n) = as_pos_f64(
                                            rv,
                                            &format!("{prefix}.burst"),
                                            &mut issues,
                                        ) {
                                            burst = n;
                                        }
                                    }
                                    "trust_x_forwarded_for" => match rv {
                                        ConfigValue::Bool(b) => trust_xff = *b,
                                        _ => issues.push(ValidationIssue {
                                            path: format!("{prefix}.{rk}"),
                                            message: "must be boolean"
                                                .to_string(),
                                        }),
                                    },
                                    _ => issues.push(ValidationIssue {
                                        path: format!("{prefix}.{rk}"),
                                        message: format!(
                                            "unknown ratelimit rule field `{rk}`"
                                        ),
                                    }),
                                }
                            }
                            cfg.rules.push(RateLimitRule {
                                prefix: rule_prefix,
                                rps,
                                burst,
                                trust_x_forwarded_for: trust_xff,
                            });
                        }
                    }
                    _ => issues.push(ValidationIssue {
                        path: k.clone(),
                        message: format!("unknown ext-ratelimit option `{k}`"),
                    }),
                }
            }

            if (global_rps.is_some() || global_burst.is_some())
                && !cfg.rules.is_empty()
            {
                if let Some(r) = global_rps {
                    cfg.rules[0].rps = r;
                }
                if let Some(b) = global_burst {
                    cfg.rules[0].burst = b;
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
            message: "ext-ratelimit config must be Null or Map".to_string(),
        }])),
    }
}

impl Plugin for RateLimitPlugin {
    fn name(&self) -> &str {
        "ext-ratelimit"
    }

    fn deps(&self) -> Deps {
        Deps::empty().with::<RouterService>()
    }

    fn schema(&self) -> PluginSchema {
        PluginSchema::new(
            "Zero-dependency Layer 3 token-bucket rate limiting filter (429 Too Many Requests)",
        )
        .field(ConfigFieldSchema::new(
            "rps",
            ConfigFieldType::Int,
            "Global tokens refilled per second (applies to default rule)",
        ))
        .field(ConfigFieldSchema::new(
            "burst",
            ConfigFieldType::Int,
            "Global token bucket burst capacity",
        ))
        .field(ConfigFieldSchema::new(
            "rules",
            ConfigFieldType::List,
            "Scoped rate limit rules ([{prefix, rps, burst, trust_x_forwarded_for}])",
        ))
        .field(
            ConfigFieldSchema::new(
                "max_entries",
                ConfigFieldType::Int,
                "Maximum client buckets cached in memory",
            )
            .with_default(ConfigValue::Int(self.config.max_entries as i64)),
        )
    }

    fn validate_config(
        &self,
        raw: &ConfigValue,
    ) -> Result<ConfigValue, FrameError> {
        let _ = resolve_ratelimit_config(&self.config, raw)?;
        Ok(raw.clone())
    }

    fn apply(
        &self,
        ctx: Context,
        config: ConfigValue,
    ) -> BoxFuture<'static, Result<(), FrameError>> {
        let base = self.config.clone();
        Box::pin(async move {
            let cfg = resolve_ratelimit_config(&base, &config)?;
            let router = ctx.use_service::<RouterService>()?;
            let _ = router.mount_filter(Arc::new(RateLimitFilter {
                rules: cfg.rules,
                max_entries: cfg.max_entries,
                buckets: Mutex::new(HashMap::new()),
            }))?;
            Ok(())
        })
    }
}
