//! BDD/TDD tests for Layer 3 zero-dependency CORS filter (`T-H07`).

use ext_cors::CorsPlugin;
use frame::{
    BoxFuture, ConfigValue, Context, EffectHandle, ErrorKind, FrameError,
    FrameOptions, Spawner,
};
use http_api::{
    FilterOutcome, HttpFilter, HttpHandler, HttpRequest, HttpResponse, Method,
    RequestMeta, RouteSpec, Router, RouterService, StatusCode,
};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context as TaskContext, Poll, Waker};

#[derive(Default)]
struct SyncSpawner;

impl Spawner for SyncSpawner {
    fn spawn(&self, mut fut: BoxFuture<'static, ()>) {
        let mut cx = TaskContext::from_waker(Waker::noop());
        let _ = Pin::new(&mut fut).poll(&mut cx);
    }
}

fn block_on<F: std::future::Future>(mut fut: F) -> F::Output {
    let mut cx = TaskContext::from_waker(Waker::noop());
    let mut pinned = unsafe { Pin::new_unchecked(&mut fut) };
    loop {
        if let Poll::Ready(v) = pinned.as_mut().poll(&mut cx) {
            return v;
        }
        std::thread::yield_now();
    }
}

struct MockRouter {
    filters: Mutex<Vec<Arc<dyn HttpFilter>>>,
}

impl Router for MockRouter {
    fn mount_route(
        &self,
        _caller: &Context,
        _spec: RouteSpec,
        _handler: Arc<dyn HttpHandler>,
    ) -> Result<EffectHandle, FrameError> {
        Err(FrameError::new(
            ErrorKind::ActivationFailed,
            "unused in mock",
        ))
    }

    fn mount_filter(
        &self,
        _caller: &Context,
        filter: Arc<dyn HttpFilter>,
    ) -> Result<EffectHandle, FrameError> {
        self.filters.lock().expect("filters").push(filter);
        Err(FrameError::new(
            ErrorKind::ActivationFailed,
            "mock filter effect unused",
        ))
    }

    fn dispatch(&self, req: HttpRequest) -> BoxFuture<'static, HttpResponse> {
        let filters = self.filters.lock().expect("filters").clone();
        Box::pin(async move {
            let meta: RequestMeta = req.request_meta();
            let mut cur = req;
            let mut resp = None;
            for f in &filters {
                match f.on_request(cur).await {
                    FilterOutcome::Continue(next) => cur = next,
                    FilterOutcome::Respond(r) => {
                        resp = Some(r);
                        break;
                    }
                }
            }
            let mut out = resp.unwrap_or_else(|| {
                HttpResponse::text(StatusCode::OK, "backend")
            });
            for f in filters.iter().rev() {
                out = f.on_response(&meta, out).await;
            }
            out
        })
    }
}

/// Real `RouterService` variant binding filter effect directly to `caller`.
struct LiveMockRouter {
    filters: Arc<Mutex<Vec<Arc<dyn HttpFilter>>>>,
}

impl Router for LiveMockRouter {
    fn mount_route(
        &self,
        _caller: &Context,
        _spec: RouteSpec,
        _handler: Arc<dyn HttpHandler>,
    ) -> Result<EffectHandle, FrameError> {
        unreachable!()
    }

    fn mount_filter(
        &self,
        caller: &Context,
        filter: Arc<dyn HttpFilter>,
    ) -> Result<EffectHandle, FrameError> {
        let filters = Arc::clone(&self.filters);
        let ptr = Arc::as_ptr(&filter) as *const () as usize;
        caller.effect_sync("mock-filter", move |scope| {
            filters.lock().expect("f").push(filter);
            let cleanup_list = Arc::clone(&filters);
            scope.defer(move || {
                cleanup_list
                    .lock()
                    .expect("f")
                    .retain(|f| (Arc::as_ptr(f) as *const () as usize) != ptr);
            });
            Ok(())
        })
    }

    fn dispatch(&self, req: HttpRequest) -> BoxFuture<'static, HttpResponse> {
        let mock = MockRouter {
            filters: Mutex::new(self.filters.lock().expect("f").clone()),
        };
        mock.dispatch(req)
    }
}

#[test]
fn cors_disabled_by_default_and_allows_when_mounted() {
    use frame::Plugin;
    let err = CorsPlugin
        .validate_config(&ConfigValue::Bool(true))
        .expect_err("reject invalid cors config");
    assert_eq!(err.kind, ErrorKind::ValidationFailed);

    let ctx = Context::new(FrameOptions::new(Arc::new(SyncSpawner)));
    let router_impl = Arc::new(LiveMockRouter {
        filters: Arc::new(Mutex::new(Vec::new())),
    });
    let svc = Arc::new(RouterService::new(router_impl));
    block_on(ctx.provide(Arc::clone(&svc))).expect("provide router");

    // Disabled by default: no CORS header
    let plain = block_on(
        svc.dispatch(
            HttpRequest::new(Method::Get, "/hello.bin")
                .with_header("Origin", "https://example.com"),
        ),
    );
    assert!(plain.header("access-control-allow-origin").is_none());

    // Mount CorsPlugin
    let plugin_handle = ctx
        .plugin(CorsPlugin, ConfigValue::Null)
        .expect("mount cors");
    block_on(ctx.settle());

    let get_resp = block_on(
        svc.dispatch(
            HttpRequest::new(Method::Get, "/hello.bin")
                .with_header("Origin", "https://example.com"),
        ),
    );
    assert_eq!(get_resp.header("access-control-allow-origin"), Some("*"));

    let preflight = block_on(
        svc.dispatch(
            HttpRequest::new(Method::Options, "/hello.bin")
                .with_header("Origin", "https://example.com")
                .with_header("Access-Control-Request-Method", "GET"),
        ),
    );
    assert_eq!(preflight.status, StatusCode::NO_CONTENT);
    assert_eq!(preflight.header("access-control-allow-origin"), Some("*"));
    assert!(
        preflight
            .header("access-control-allow-methods")
            .is_some_and(|v| v.contains("GET") && v.contains("OPTIONS"))
    );

    // Unloading CorsPlugin removes CORS filter cleanly
    block_on(plugin_handle.dispose()).expect("dispose cors");
    let after = block_on(
        svc.dispatch(
            HttpRequest::new(Method::Get, "/hello.bin")
                .with_header("Origin", "https://example.com"),
        ),
    );
    assert!(after.header("access-control-allow-origin").is_none());

    block_on(ctx.dispose()).expect("ctx dispose");
}
