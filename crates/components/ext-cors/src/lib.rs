//! Layer 3 pure-std CORS filter plugin.

use frame::{
    BoxFuture, ConfigValue, Context, Deps, FrameError, Plugin, ValidationIssue,
};
use http_api::{
    FilterOutcome, HttpFilter, HttpRequest, HttpResponse, Method, RequestMeta,
    RouterService, StatusCode,
};
use std::sync::Arc;

struct CorsFilter {
    allow_origin: String,
}

impl HttpFilter for CorsFilter {
    fn name(&self) -> &str {
        "cors"
    }

    fn priority(&self) -> i32 {
        -10
    }

    fn on_request(
        &self,
        req: HttpRequest,
    ) -> BoxFuture<'static, FilterOutcome> {
        let is_options = req.method == Method::Options;
        let allow_origin = self.allow_origin.clone();
        Box::pin(async move {
            if is_options {
                let resp = HttpResponse::empty(StatusCode::NO_CONTENT)
                    .with_header("Access-Control-Allow-Origin", allow_origin)
                    .with_header(
                        "Access-Control-Allow-Methods",
                        "GET, HEAD, OPTIONS",
                    )
                    .with_header("Access-Control-Allow-Headers", "*")
                    .with_header("Access-Control-Max-Age", "86400");
                FilterOutcome::Respond(resp)
            } else {
                FilterOutcome::Continue(req)
            }
        })
    }

    fn on_response(
        &self,
        _req_meta: &RequestMeta,
        mut resp: HttpResponse,
    ) -> BoxFuture<'static, HttpResponse> {
        resp.headers
            .insert("Access-Control-Allow-Origin", self.allow_origin.clone());
        Box::pin(async { resp })
    }
}

fn resolve_allow_origin(raw: &ConfigValue) -> Result<String, FrameError> {
    match raw {
        ConfigValue::Null => Ok("*".to_string()),
        ConfigValue::Map(map) => {
            let mut allow_origin = "*".to_string();
            let mut issues = Vec::new();
            for (k, v) in map {
                match k.as_str() {
                    "allow_origin" => match v {
                        ConfigValue::Str(s) if !s.trim().is_empty() => {
                            allow_origin = s.trim().to_string();
                        }
                        _ => issues.push(ValidationIssue {
                            path: k.clone(),
                            message: "allow_origin must be a non-empty string"
                                .to_string(),
                        }),
                    },
                    _ => issues.push(ValidationIssue {
                        path: k.clone(),
                        message: format!("unknown ext-cors option `{k}`"),
                    }),
                }
            }
            if issues.is_empty() {
                Ok(allow_origin)
            } else {
                Err(FrameError::validation(issues))
            }
        }
        _ => Err(FrameError::validation(vec![ValidationIssue {
            path: "".to_string(),
            message: "ext-cors config must be Null or Map".to_string(),
        }])),
    }
}

/// Plugin mounting permissive CORS headers and `OPTIONS` preflight handling.
#[derive(Debug, Clone, Copy, Default)]
pub struct CorsPlugin;

impl Plugin for CorsPlugin {
    fn name(&self) -> &str {
        "ext-cors"
    }

    fn deps(&self) -> Deps {
        Deps::empty().with::<RouterService>()
    }

    fn validate_config(
        &self,
        raw: &ConfigValue,
    ) -> Result<ConfigValue, FrameError> {
        let _ = resolve_allow_origin(raw)?;
        Ok(raw.clone())
    }

    fn apply(
        &self,
        ctx: Context,
        config: ConfigValue,
    ) -> BoxFuture<'static, Result<(), FrameError>> {
        Box::pin(async move {
            let allow_origin = resolve_allow_origin(&config)?;
            let router = ctx.use_service::<RouterService>()?;
            let _ =
                router.mount_filter(Arc::new(CorsFilter { allow_origin }))?;
            Ok(())
        })
    }
}
