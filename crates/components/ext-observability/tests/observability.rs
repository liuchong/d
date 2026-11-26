//! Integration tests for `ext-observability` (`GA15`):
//! access logs (JSON/text), `/healthz`, `/readyz`, and `/metrics`.

use d_ext_observability::{
    AccessLogFormat, ObservabilityConfig, ObservabilityPlugin,
};
use frame::{
    BoxFuture, ConfigValue, Context, Deps, EffectHandle, FrameError,
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
use std::time::{Instant, SystemTime, UNIX_EPOCH};

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
                is_tls: req.is_tls,
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
        Box::pin(async { HttpResponse::text(StatusCode::OK, "pong") })
    }
}

#[test]
fn observability_logs_metrics_and_probes_work() {
    let bad = ObservabilityPlugin::default()
        .validate_config(&ConfigValue::Map(BTreeMap::from([(
            "log_format".to_string(),
            ConfigValue::Str("xml".to_string()),
        )])))
        .expect_err("invalid format must fail");
    assert_eq!(bad.kind, frame::ErrorKind::ValidationFailed);

    let uniq = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let log_path = std::env::temp_dir()
        .join(format!("d-obs-{}-{uniq}.log", std::process::id()));

    let ctx = Context::new(FrameOptions::new(Arc::new(StdSpawner)));
    block_on_ready(ctx.provide(Arc::new(RouterService::new(Arc::new(
        TestRouter::default(),
    )))))
    .unwrap();
    ctx.plugin(
        ObservabilityPlugin::new(ObservabilityConfig {
            log_format: AccessLogFormat::Json,
            log_file: Some(log_path.clone()),
            ..ObservabilityConfig::default()
        }),
        ConfigValue::Null,
    )
    .unwrap();
    block_on_ready(ctx.settle());

    let router = ctx.get::<RouterService>().unwrap().as_ref().clone();
    let _r = router
        .mount_route(
            &ctx,
            RouteSpec::new("ping", RouteMatch::Exact("/ping".into())),
            Arc::new(OkHandler),
        )
        .unwrap();

    let h = block_on_ready(
        router.dispatch(HttpRequest::new(Method::Get, "/healthz")),
    );
    assert_eq!(h.status, StatusCode::OK);
    let rdy = block_on_ready(
        router.dispatch(HttpRequest::new(Method::Get, "/readyz")),
    );
    assert_eq!(rdy.status, StatusCode::OK);

    let ping =
        block_on_ready(router.dispatch(HttpRequest::new(Method::Get, "/ping")));
    assert_eq!(ping.status, StatusCode::OK);
    let miss =
        block_on_ready(router.dispatch(HttpRequest::new(Method::Get, "/nope")));
    assert_eq!(miss.status, StatusCode::NOT_FOUND);

    let metrics_resp = block_on_ready(
        router.dispatch(HttpRequest::new(Method::Get, "/metrics")),
    );
    assert_eq!(metrics_resp.status, StatusCode::OK);
    let metrics_txt = String::from_utf8(
        block_on_ready(metrics_resp.body.into_bytes()).unwrap(),
    )
    .unwrap();
    assert!(
        metrics_txt.contains("d_http_requests_total{class=\"2xx\"}"),
        "{metrics_txt}"
    );
    assert!(
        metrics_txt.contains("d_http_requests_total{class=\"4xx\"}"),
        "{metrics_txt}"
    );
    assert!(
        metrics_txt.contains("d_http_request_duration_ms_bucket{le=\"+Inf\"}"),
        "{metrics_txt}"
    );
    assert!(metrics_txt.contains("d_fibers_active"), "{metrics_txt}");

    let written_log = std::fs::read_to_string(&log_path).unwrap();
    let _ = std::fs::remove_file(&log_path);
    assert!(written_log.contains("\"path\":\"/ping\""), "{written_log}");
    assert!(written_log.contains("\"status\":200"), "{written_log}");

    // Mount a failing fiber and verify /readyz returns 503.
    let bad_fiber = ctx
        .inject("broken", Deps::empty(), |_| {
            Box::pin(async {
                Err(frame::FrameError::new(
                    frame::ErrorKind::ActivationFailed,
                    "boom",
                ))
            })
        })
        .unwrap();
    let _ = block_on_ready(bad_fiber.wait());
    let not_ready = block_on_ready(
        router.dispatch(HttpRequest::new(Method::Get, "/readyz")),
    );
    assert_eq!(not_ready.status, StatusCode::SERVICE_UNAVAILABLE);

    block_on_ready(ctx.dispose()).unwrap();
}
