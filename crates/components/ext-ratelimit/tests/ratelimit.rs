//! Integration tests for `ext-ratelimit` (`GA16`):
//! token-bucket allowance, 429 Too Many Requests + Retry-After, and refill recovery.

use ext_ratelimit::{RateLimitConfig, RateLimitPlugin, RateLimitRule};
use frame::{
    BoxFuture, ConfigValue, Context, EffectHandle, ErrorKind, FrameError,
    FrameOptions, Plugin, Spawner,
};
use http_api::{
    FilterOutcome, HttpFilter, HttpHandler, HttpRequest, HttpResponse, Method,
    RequestMeta, RouteMatch, RouteSpec, Router, RouterService, StatusCode,
};
use std::collections::BTreeMap;
use std::future::Future;
use std::pin::pin;
use std::sync::{Arc, Mutex};
use std::task::{Context as TaskContext, Poll, Waker};
use std::time::{Duration, Instant};

fn block_on_ready<F: Future>(fut: F) -> F::Output {
    let mut fut = pin!(fut);
    let mut cx = TaskContext::from_waker(Waker::noop());
    for _ in 0..2048 {
        if let Poll::Ready(out) = fut.as_mut().poll(&mut cx) {
            return out;
        }
        std::thread::yield_now();
    }
    panic!("future did not complete within 2048 polls");
}

#[derive(Debug, Default)]
struct StdSpawner;
impl Spawner for StdSpawner {
    fn spawn(&self, fut: BoxFuture<'static, ()>) {
        block_on_ready(fut);
    }
}

#[derive(Default)]
struct TestRouter {
    routes: Mutex<Vec<(RouteSpec, Arc<dyn HttpHandler>)>>,
    filters: Mutex<Vec<Arc<dyn HttpFilter>>>,
}

impl Router for TestRouter {
    fn mount_route(
        &self,
        caller: &Context,
        spec: RouteSpec,
        handler: Arc<dyn HttpHandler>,
    ) -> Result<EffectHandle, FrameError> {
        let id = spec.id.clone();
        self.routes.lock().unwrap().push((spec, handler));
        caller.effect_sync(format!("route:{id}"), |_| Ok(()))
    }

    fn mount_filter(
        &self,
        caller: &Context,
        filter: Arc<dyn HttpFilter>,
    ) -> Result<EffectHandle, FrameError> {
        let name = filter.name().to_string();
        let mut g = self.filters.lock().unwrap();
        g.push(filter);
        g.sort_by_key(|f| f.priority());
        caller.effect_sync(format!("filter:{name}"), |_| Ok(()))
    }

    fn dispatch(&self, req: HttpRequest) -> BoxFuture<'static, HttpResponse> {
        let routes = self.routes.lock().unwrap().clone();
        let filters = self.filters.lock().unwrap().clone();
        Box::pin(async move {
            let meta = RequestMeta {
                is_head: req.method == Method::Head,
                method: req.method.clone(),
                path: req.path.clone(),
                query: req.query.clone(),
                version: req.version,
                headers: req.headers.clone(),
                remote_addr: req.remote_addr,
                started_at: Instant::now(),
            };
            let mut ran = Vec::new();
            let mut short_circuit = None;
            let mut cur = Some(req);
            for f in &filters {
                ran.push(Arc::clone(f));
                let next_req = cur.take().unwrap();
                match f.on_request(next_req).await {
                    FilterOutcome::Continue(next) => cur = Some(next),
                    FilterOutcome::Respond(resp) => {
                        short_circuit = Some(resp);
                        break;
                    }
                }
            }
            let mut resp = if let Some(r) = short_circuit {
                r
            } else {
                let req = cur.unwrap();
                let req_path = req.path.clone();
                if let Some((_, h)) =
                    routes.into_iter().find(|(spec, _)| match &spec.matcher {
                        RouteMatch::Exact(p) => req_path == *p,
                        RouteMatch::Prefix(p) => req_path.starts_with(p),
                        RouteMatch::CatchAll => true,
                        RouteMatch::Param(_) => false,
                    })
                {
                    h.call(req).await
                } else {
                    HttpResponse::text(StatusCode::NOT_FOUND, "not found")
                }
            };
            for f in ran.into_iter().rev() {
                resp = f.on_response(&meta, resp).await;
            }
            resp
        })
    }
}

struct OkHandler;
impl HttpHandler for OkHandler {
    fn call(&self, _req: HttpRequest) -> BoxFuture<'static, HttpResponse> {
        Box::pin(async { HttpResponse::text(StatusCode::OK, "ok") })
    }
}

#[test]
fn ratelimit_enforces_burst_and_recovers_after_refill() {
    let bad = RateLimitPlugin::default()
        .validate_config(&ConfigValue::Map(BTreeMap::from([(
            "rps".to_string(),
            ConfigValue::Int(0),
        )])))
        .expect_err("zero rps must fail");
    assert_eq!(bad.kind, ErrorKind::ValidationFailed);

    let ctx = Context::new(FrameOptions::new(Arc::new(StdSpawner)));
    block_on_ready(ctx.provide(Arc::new(RouterService::new(Arc::new(
        TestRouter::default(),
    )))))
    .unwrap();
    ctx.plugin(
        RateLimitPlugin::new(RateLimitConfig {
            rules: vec![RateLimitRule {
                prefix: "/api".to_string(),
                rps: 50.0,
                burst: 2.0,
                trust_x_forwarded_for: true,
            }],
            max_entries: 64,
        }),
        ConfigValue::Null,
    )
    .unwrap();
    block_on_ready(ctx.settle());

    let router = ctx.get::<RouterService>().unwrap().as_ref().clone();
    let _r = router
        .mount_route(
            &ctx,
            RouteSpec::new("api", RouteMatch::Exact("/api/test".into())),
            Arc::new(OkHandler),
        )
        .unwrap();

    // First 2 requests from client-A succeed, 3rd triggers 429 + Retry-After.
    for _ in 0..2 {
        let req = HttpRequest::new(Method::Get, "/api/test")
            .with_header("X-Forwarded-For", "198.51.100.7");
        assert_eq!(block_on_ready(router.dispatch(req)).status, StatusCode::OK);
    }
    let limited = block_on_ready(
        router.dispatch(
            HttpRequest::new(Method::Get, "/api/test")
                .with_header("X-Forwarded-For", "198.51.100.7"),
        ),
    );
    assert_eq!(limited.status, StatusCode::TOO_MANY_REQUESTS);
    assert!(limited.header("retry-after").is_some());

    // Different client IP has independent bucket.
    let other_client = block_on_ready(
        router.dispatch(
            HttpRequest::new(Method::Get, "/api/test")
                .with_header("X-Forwarded-For", "198.51.100.8"),
        ),
    );
    assert_eq!(other_client.status, StatusCode::OK);

    // Wait > 25ms for token refill at 50 rps (>= 1.25 tokens).
    std::thread::sleep(Duration::from_millis(30));
    let recovered = block_on_ready(
        router.dispatch(
            HttpRequest::new(Method::Get, "/api/test")
                .with_header("X-Forwarded-For", "198.51.100.7"),
        ),
    );
    assert_eq!(recovered.status, StatusCode::OK);

    block_on_ready(ctx.dispose()).unwrap();
}
