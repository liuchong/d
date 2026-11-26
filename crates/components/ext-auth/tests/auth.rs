//! Integration tests for `ext-auth` (`GA17`):
//! Bearer tokens, Basic credentials, IP CIDR allow/deny, and secret schema flags.

use d_ext_auth::{AuthConfig, AuthPlugin, AuthRule, IpCidr};
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
use std::time::Instant;

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
        Box::pin(async { HttpResponse::text(StatusCode::OK, "secret-ok") })
    }
}

#[test]
fn auth_bearer_basic_ip_cidr_and_secret_schema() {
    let schema = AuthPlugin::default().schema();
    assert!(
        schema
            .fields
            .iter()
            .any(|f| f.name == "rules" && f.sensitive),
        "auth rules must be marked secret in schema"
    );

    let bad = AuthPlugin::default()
        .validate_config(&ConfigValue::Map(BTreeMap::from([(
            "rules".to_string(),
            ConfigValue::List(vec![ConfigValue::Map(BTreeMap::from([(
                "ip_allow".to_string(),
                ConfigValue::List(vec![ConfigValue::Str(
                    "999.0.0.1/99".to_string(),
                )]),
            )]))]),
        )])))
        .expect_err("bad CIDR must reject");
    assert_eq!(bad.kind, ErrorKind::ValidationFailed);

    let ctx = Context::new(FrameOptions::new(Arc::new(StdSpawner)));
    block_on_ready(ctx.provide(Arc::new(RouterService::new(Arc::new(
        TestRouter::default(),
    )))))
    .unwrap();
    ctx.plugin(
        AuthPlugin::new(AuthConfig {
            rules: vec![AuthRule {
                prefix: "/admin".to_string(),
                host: None,
                bearer_tokens: vec!["tok-secret".to_string()],
                // base64("alice:pass123") == "YWxpY2U6cGFzczEyMw=="
                basic_users: vec!["alice:pass123".to_string()],
                ip_allow: vec![IpCidr::parse("127.0.0.0/8").unwrap()],
                ip_deny: vec![IpCidr::parse("127.0.0.9/32").unwrap()],
            }],
        }),
        ConfigValue::Null,
    )
    .unwrap();
    block_on_ready(ctx.settle());

    let router = ctx.get::<RouterService>().unwrap().as_ref().clone();
    let _r = router
        .mount_route(
            &ctx,
            RouteSpec::new("admin", RouteMatch::Exact("/admin/dash".into())),
            Arc::new(OkHandler),
        )
        .unwrap();
    let _pub_r = router
        .mount_route(
            &ctx,
            RouteSpec::new("pub", RouteMatch::Exact("/public".into())),
            Arc::new(OkHandler),
        )
        .unwrap();

    // Unmatched public route passes without credentials.
    assert_eq!(
        block_on_ready(
            router.dispatch(HttpRequest::new(Method::Get, "/public"))
        )
        .status,
        StatusCode::OK
    );

    // Denied IP gets 403 even with valid token.
    let mut denied = HttpRequest::new(Method::Get, "/admin/dash")
        .with_header("Authorization", "Bearer tok-secret");
    denied.remote_addr = Some("127.0.0.9:5000".parse().unwrap());
    assert_eq!(
        block_on_ready(router.dispatch(denied)).status,
        StatusCode::FORBIDDEN
    );

    // Missing credentials -> 401.
    let mut no_cred = HttpRequest::new(Method::Get, "/admin/dash");
    no_cred.remote_addr = Some("127.0.0.1:5000".parse().unwrap());
    assert_eq!(
        block_on_ready(router.dispatch(no_cred)).status,
        StatusCode::UNAUTHORIZED
    );

    // Valid Bearer -> 200.
    let mut ok_bearer = HttpRequest::new(Method::Get, "/admin/dash")
        .with_header("Authorization", "Bearer tok-secret");
    ok_bearer.remote_addr = Some("127.0.0.1:5000".parse().unwrap());
    assert_eq!(
        block_on_ready(router.dispatch(ok_bearer)).status,
        StatusCode::OK
    );

    // Valid Basic -> 200.
    let mut ok_basic = HttpRequest::new(Method::Get, "/admin/dash")
        .with_header("Authorization", "Basic YWxpY2U6cGFzczEyMw==");
    ok_basic.remote_addr = Some("127.0.0.1:5000".parse().unwrap());
    assert_eq!(
        block_on_ready(router.dispatch(ok_basic)).status,
        StatusCode::OK
    );

    // Runtime inspect snapshot never contains token/password strings.
    let snap = format!("{:?}", ctx.inspect());
    assert!(!snap.contains("tok-secret"));
    assert!(!snap.contains("pass123"));

    block_on_ready(ctx.dispose()).unwrap();
}
