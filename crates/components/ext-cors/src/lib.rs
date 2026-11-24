//! Layer 3 pure-std CORS filter plugin.

use frame::{BoxFuture, ConfigValue, Context, Deps, FrameError, Plugin};
use http_api::{
    FilterOutcome, HttpFilter, HttpRequest, HttpResponse, Method, RequestMeta,
    RouterService, StatusCode,
};
use std::sync::Arc;

struct CorsFilter;

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
        Box::pin(async move {
            if is_options {
                let resp = HttpResponse::empty(StatusCode::NO_CONTENT)
                    .with_header("Access-Control-Allow-Origin", "*")
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
        resp.headers.insert("Access-Control-Allow-Origin", "*");
        Box::pin(async { resp })
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

    fn apply(
        &self,
        ctx: Context,
        _config: ConfigValue,
    ) -> BoxFuture<'static, Result<(), FrameError>> {
        Box::pin(async move {
            let router = ctx.require::<RouterService>()?;
            let filter_handle = router.mount_filter(Arc::new(CorsFilter))?;
            let _ = ctx.effect_sync("ext-cors-mount", move |scope| {
                scope.defer_async(move || filter_handle.dispose());
                Ok(())
            })?;
            Ok(())
        })
    }
}
