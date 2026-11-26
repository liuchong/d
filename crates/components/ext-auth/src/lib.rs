//! Zero-dependency Layer 3 Bearer token, Basic auth, and IP CIDR access control.

use frame::{
    BoxFuture, ConfigFieldSchema, ConfigFieldType, ConfigValue, Context, Deps,
    FrameError, Plugin, PluginSchema, ValidationIssue,
};
use http_api::{
    FilterOutcome, HttpFilter, HttpRequest, HttpResponse, RequestMeta,
    RouterService, StatusCode,
};
use std::net::IpAddr;
use std::sync::Arc;

/// Parsed IPv4/IPv6 CIDR network filter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IpCidr {
    addr: IpAddr,
    prefix_len: u8,
}

impl IpCidr {
    /// Parse `10.0.0.0/8`, `127.0.0.1`, or IPv6 CIDR notation.
    pub fn parse(raw: &str) -> Result<Self, String> {
        let trimmed = raw.trim();
        if let Some((ip_str, plen_str)) = trimmed.split_once('/') {
            let addr: IpAddr = ip_str
                .parse()
                .map_err(|e| format!("invalid IP `{ip_str}`: {e}"))?;
            let max_bits = match addr {
                IpAddr::V4(_) => 32,
                IpAddr::V6(_) => 128,
            };
            let prefix_len: u8 = plen_str.parse().map_err(|e| {
                format!("invalid CIDR prefix `{plen_str}`: {e}")
            })?;
            if prefix_len > max_bits {
                return Err(format!(
                    "CIDR prefix /{prefix_len} exceeds max /{max_bits}"
                ));
            }
            Ok(Self { addr, prefix_len })
        } else {
            let addr: IpAddr = trimmed
                .parse()
                .map_err(|e| format!("invalid IP `{trimmed}`: {e}"))?;
            let prefix_len = match addr {
                IpAddr::V4(_) => 32,
                IpAddr::V6(_) => 128,
            };
            Ok(Self { addr, prefix_len })
        }
    }

    /// Return `true` when `ip` matches this CIDR subnet.
    pub fn contains(&self, ip: IpAddr) -> bool {
        match (self.addr, ip) {
            (IpAddr::V4(net), IpAddr::V4(cand)) => {
                if self.prefix_len == 0 {
                    return true;
                }
                let mask = !0u32 << (32 - self.prefix_len);
                (u32::from(net) & mask) == (u32::from(cand) & mask)
            }
            (IpAddr::V6(net), IpAddr::V6(cand)) => {
                if self.prefix_len == 0 {
                    return true;
                }
                let mask = !0u128 << (128 - self.prefix_len);
                (u128::from(net) & mask) == (u128::from(cand) & mask)
            }
            _ => false,
        }
    }
}

/// Scoped authentication and IP rule.
#[derive(Debug, Clone)]
pub struct AuthRule {
    pub prefix: String,
    pub host: Option<String>,
    pub bearer_tokens: Vec<String>,
    pub basic_users: Vec<String>,
    pub ip_allow: Vec<IpCidr>,
    pub ip_deny: Vec<IpCidr>,
}

/// Top-level configuration for [`AuthPlugin`].
#[derive(Debug, Clone, Default)]
pub struct AuthConfig {
    pub rules: Vec<AuthRule>,
}

/// Constant-time slice equality avoiding timing leaks on token/credential checks.
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    let max_len = a.len().max(b.len());
    let mut diff: u8 = u8::from(a.len() != b.len());
    for i in 0..max_len {
        let ba = a.get(i).copied().unwrap_or(0);
        let bb = b.get(i).copied().unwrap_or(0);
        diff |= ba ^ bb;
    }
    diff == 0
}

fn decode_base64(input: &str) -> Option<String> {
    fn val(b: u8) -> Option<u8> {
        match b {
            b'A'..=b'Z' => Some(b - b'A'),
            b'a'..=b'z' => Some(b - b'a' + 26),
            b'0'..=b'9' => Some(b - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }

    let bytes = input.trim().as_bytes();
    if bytes.is_empty() || !bytes.len().is_multiple_of(4) {
        return None;
    }
    let mut out = Vec::with_capacity((bytes.len() / 4) * 3);
    for chunk in bytes.as_chunks::<4>().0 {
        let a = val(chunk[0])?;
        let b = val(chunk[1])?;
        let pad_c = chunk[2] == b'=';
        let pad_d = chunk[3] == b'=';
        let c = if pad_c { 0 } else { val(chunk[2])? };
        let d = if pad_d { 0 } else { val(chunk[3])? };
        out.push((a << 2) | (b >> 4));
        if !pad_c {
            out.push((b << 4) | (c >> 2));
        }
        if !pad_d {
            out.push((c << 6) | d);
        }
    }
    String::from_utf8(out).ok()
}

struct AuthFilter {
    rules: Vec<AuthRule>,
}

fn rule_matches(rule: &AuthRule, req: &HttpRequest) -> bool {
    if !req.path.starts_with(&rule.prefix) {
        return false;
    }
    if let Some(expected_host) = &rule.host {
        let Some(actual) = req.headers.get("host") else {
            return false;
        };
        let norm = if expected_host.contains(':') {
            actual
        } else {
            actual.split(':').next().unwrap_or(actual)
        };
        if !norm.eq_ignore_ascii_case(expected_host) {
            return false;
        }
    }
    true
}

impl HttpFilter for AuthFilter {
    fn name(&self) -> &str {
        "ext-auth"
    }

    fn priority(&self) -> i32 {
        -600
    }

    fn on_request(
        &self,
        req: HttpRequest,
    ) -> BoxFuture<'static, FilterOutcome> {
        let matched = self
            .rules
            .iter()
            .filter(|r| rule_matches(r, &req))
            .max_by_key(|r| r.prefix.len());

        let Some(rule) = matched else {
            return Box::pin(async move { FilterOutcome::Continue(req) });
        };

        let remote_ip = req.remote_addr.map(|a| a.ip());
        if !rule.ip_deny.is_empty()
            && let Some(ip) = remote_ip
            && rule.ip_deny.iter().any(|c| c.contains(ip))
        {
            return Box::pin(async move {
                FilterOutcome::Respond(HttpResponse::text(
                    StatusCode::FORBIDDEN,
                    "Forbidden by IP policy",
                ))
            });
        }
        if !rule.ip_allow.is_empty() {
            let allowed = remote_ip
                .is_some_and(|ip| rule.ip_allow.iter().any(|c| c.contains(ip)));
            if !allowed {
                return Box::pin(async move {
                    FilterOutcome::Respond(HttpResponse::text(
                        StatusCode::FORBIDDEN,
                        "Forbidden: client IP not in allowlist",
                    ))
                });
            }
        }

        let need_cred =
            !rule.bearer_tokens.is_empty() || !rule.basic_users.is_empty();
        if !need_cred {
            return Box::pin(async move { FilterOutcome::Continue(req) });
        }

        let auth_hdr = req.headers.get("authorization").unwrap_or("");
        let mut authorized = false;

        if let Some(tok) = auth_hdr.strip_prefix("Bearer ")
            && !rule.bearer_tokens.is_empty()
        {
            for expected in &rule.bearer_tokens {
                if constant_time_eq(tok.trim().as_bytes(), expected.as_bytes())
                {
                    authorized = true;
                }
            }
        } else if let Some(b64) = auth_hdr.strip_prefix("Basic ")
            && !rule.basic_users.is_empty()
            && let Some(decoded) = decode_base64(b64)
        {
            for expected in &rule.basic_users {
                if constant_time_eq(decoded.as_bytes(), expected.as_bytes()) {
                    authorized = true;
                }
            }
        }

        if authorized {
            Box::pin(async move { FilterOutcome::Continue(req) })
        } else {
            let realm = if !rule.bearer_tokens.is_empty() {
                "Bearer realm=\"d\""
            } else {
                "Basic realm=\"d\""
            };
            Box::pin(async move {
                FilterOutcome::Respond(
                    HttpResponse::text(
                        StatusCode::UNAUTHORIZED,
                        "Unauthorized",
                    )
                    .with_header("WWW-Authenticate", realm),
                )
            })
        }
    }

    fn on_response(
        &self,
        _req_meta: &RequestMeta,
        resp: HttpResponse,
    ) -> BoxFuture<'static, HttpResponse> {
        Box::pin(async move { resp })
    }
}

/// Layer 3 plugin enforcing Bearer/Basic credentials and IP CIDR rules.
#[derive(Debug, Clone, Default)]
pub struct AuthPlugin {
    config: AuthConfig,
}

impl AuthPlugin {
    /// Construct an auth plugin with programmatic default rules.
    pub fn new(config: AuthConfig) -> Self {
        Self { config }
    }
}

fn parse_str_list(
    v: &ConfigValue,
    path: &str,
    issues: &mut Vec<ValidationIssue>,
) -> Vec<String> {
    let ConfigValue::List(arr) = v else {
        issues.push(ValidationIssue {
            path: path.to_string(),
            message: "must be a list of strings".to_string(),
        });
        return Vec::new();
    };
    let mut out = Vec::with_capacity(arr.len());
    for (idx, item) in arr.iter().enumerate() {
        match item {
            ConfigValue::Str(s) if !s.is_empty() => out.push(s.clone()),
            _ => issues.push(ValidationIssue {
                path: format!("{path}[{idx}]"),
                message: "item must be a non-empty string".to_string(),
            }),
        }
    }
    out
}

fn parse_cidr_list(
    v: &ConfigValue,
    path: &str,
    issues: &mut Vec<ValidationIssue>,
) -> Vec<IpCidr> {
    let raw = parse_str_list(v, path, issues);
    let mut out = Vec::with_capacity(raw.len());
    for (idx, s) in raw.into_iter().enumerate() {
        match IpCidr::parse(&s) {
            Ok(c) => out.push(c),
            Err(e) => issues.push(ValidationIssue {
                path: format!("{path}[{idx}]"),
                message: e,
            }),
        }
    }
    out
}

fn resolve_auth_config(
    base: &AuthConfig,
    raw: &ConfigValue,
) -> Result<AuthConfig, FrameError> {
    let mut cfg = base.clone();
    match raw {
        ConfigValue::Null => Ok(cfg),
        ConfigValue::Map(map) => {
            let mut issues = Vec::new();
            for (k, v) in map {
                match k.as_str() {
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
                                    message: "auth rule must be a map"
                                        .to_string(),
                                });
                                continue;
                            };
                            let mut rule = AuthRule {
                                prefix: "/".to_string(),
                                host: None,
                                bearer_tokens: Vec::new(),
                                basic_users: Vec::new(),
                                ip_allow: Vec::new(),
                                ip_deny: Vec::new(),
                            };
                            for (rk, rv) in rmap {
                                match rk.as_str() {
                                    "prefix" => match rv {
                                        ConfigValue::Str(s)
                                            if s.starts_with('/') =>
                                        {
                                            rule.prefix = s.clone();
                                        }
                                        _ => issues.push(ValidationIssue {
                                            path: format!("{prefix}.prefix"),
                                            message:
                                                "prefix must start with `/`"
                                                    .to_string(),
                                        }),
                                    },
                                    "host" => match rv {
                                        ConfigValue::Null => rule.host = None,
                                        ConfigValue::Str(s) => {
                                            rule.host = Some(s.clone());
                                        }
                                        _ => issues.push(ValidationIssue {
                                            path: format!("{prefix}.host"),
                                            message:
                                                "host must be string or null"
                                                    .to_string(),
                                        }),
                                    },
                                    "bearer_tokens" => {
                                        rule.bearer_tokens = parse_str_list(
                                            rv,
                                            &format!("{prefix}.bearer_tokens"),
                                            &mut issues,
                                        );
                                    }
                                    "basic_users" => {
                                        rule.basic_users = parse_str_list(
                                            rv,
                                            &format!("{prefix}.basic_users"),
                                            &mut issues,
                                        );
                                    }
                                    "ip_allow" => {
                                        rule.ip_allow = parse_cidr_list(
                                            rv,
                                            &format!("{prefix}.ip_allow"),
                                            &mut issues,
                                        );
                                    }
                                    "ip_deny" => {
                                        rule.ip_deny = parse_cidr_list(
                                            rv,
                                            &format!("{prefix}.ip_deny"),
                                            &mut issues,
                                        );
                                    }
                                    _ => issues.push(ValidationIssue {
                                        path: format!("{prefix}.{rk}"),
                                        message: format!(
                                            "unknown ext-auth rule field `{rk}`"
                                        ),
                                    }),
                                }
                            }
                            cfg.rules.push(rule);
                        }
                    }
                    _ => issues.push(ValidationIssue {
                        path: k.clone(),
                        message: format!("unknown ext-auth option `{k}`"),
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
            message: "ext-auth config must be Null or Map".to_string(),
        }])),
    }
}

impl Plugin for AuthPlugin {
    fn name(&self) -> &str {
        "ext-auth"
    }

    fn deps(&self) -> Deps {
        Deps::empty().with::<RouterService>()
    }

    fn schema(&self) -> PluginSchema {
        PluginSchema::new(
            "Zero-dependency Layer 3 Bearer, Basic, and IP CIDR authentication filter (401/403)",
        )
        .field(
            ConfigFieldSchema::new(
                "rules",
                ConfigFieldType::List,
                "Auth rules ([{prefix, host, bearer_tokens, basic_users, ip_allow, ip_deny}])",
            )
            .sensitive(),
        )
    }

    fn validate_config(
        &self,
        raw: &ConfigValue,
    ) -> Result<ConfigValue, FrameError> {
        let _ = resolve_auth_config(&self.config, raw)?;
        Ok(raw.clone())
    }

    fn apply(
        &self,
        ctx: Context,
        config: ConfigValue,
    ) -> BoxFuture<'static, Result<(), FrameError>> {
        let base = self.config.clone();
        Box::pin(async move {
            let cfg = resolve_auth_config(&base, &config)?;
            let router = ctx.use_service::<RouterService>()?;
            let _ = router
                .mount_filter(Arc::new(AuthFilter { rules: cfg.rules }))?;
            Ok(())
        })
    }
}
