use d_daemon::{
    AssemblySpec, RuntimeControlService, build_from_spec,
    load_entries_from_json_file, parse_entries_from_json,
};
use frame::{
    BoxFuture, ConfigValue, Context, Deps, EffectHandle, EntryTarget,
    ErrorKind, FrameError, Plugin, ServiceKey, StaticResolver,
};
use http_api::{
    HttpFilter, HttpHandler, HttpRequest, HttpResponse, RouteSpec, Router,
    RouterService, StatusCode,
};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Default)]
struct StubRouter;

impl Router for StubRouter {
    fn mount_route(
        &self,
        caller: &Context,
        spec: RouteSpec,
        _handler: Arc<dyn HttpHandler>,
    ) -> Result<EffectHandle, FrameError> {
        caller.effect_sync(format!("route:{}", spec.id), |_| Ok(()))
    }

    fn mount_filter(
        &self,
        caller: &Context,
        filter: Arc<dyn HttpFilter>,
    ) -> Result<EffectHandle, FrameError> {
        caller.effect_sync(format!("filter:{}", filter.name()), |_| Ok(()))
    }

    fn dispatch(&self, _req: HttpRequest) -> BoxFuture<'static, HttpResponse> {
        Box::pin(async { HttpResponse::text(StatusCode::NOT_FOUND, "stub") })
    }
}

#[derive(Default)]
struct DummyOkPlugin;

impl Plugin for DummyOkPlugin {
    fn name(&self) -> &str {
        "dummy-ok"
    }

    fn apply(
        &self,
        ctx: frame::Context,
        _config: ConfigValue,
    ) -> frame::BoxFuture<'static, Result<(), FrameError>> {
        Box::pin(async move {
            ctx.provide(Arc::new(RouterService::new(Arc::new(StubRouter))))
                .await?;
            Ok(())
        })
    }
}

#[test]
fn parses_manifest_array_and_plugins_object_and_rejects_invalid_json() {
    let raw_obj = r#"{
        "plugins": [
            {
                "id": "group-a",
                "target": {"kind": "group"},
                "isolate": ["http-api"],
                "children": [
                    {
                        "id": "sub",
                        "target": "dummy-ok",
                        "disabled": true,
                        "config": {"count": 2, "flag": true}
                    }
                ]
            }
        ]
    }"#;
    let parsed =
        parse_entries_from_json(raw_obj).expect("valid object manifest");
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].id, "group-a");
    assert_eq!(parsed[0].target, EntryTarget::Group);
    assert_eq!(
        parsed[0].isolate,
        vec![ServiceKey::of::<http_api::RouterService>()]
    );
    assert_eq!(parsed[0].children.len(), 1);
    assert!(parsed[0].children[0].disabled);
    assert_eq!(
        parsed[0].children[0].target,
        EntryTarget::Name("dummy-ok".to_string())
    );

    let raw_arr = r#"[{"id":"dummy-ok"}]"#;
    let arr_entries = parse_entries_from_json(raw_arr).expect("array manifest");
    assert_eq!(arr_entries.len(), 1);
    assert_eq!(
        arr_entries[0].target,
        EntryTarget::Name("dummy-ok".to_string())
    );

    assert_eq!(
        parse_entries_from_json("{bad json").unwrap_err().kind,
        ErrorKind::ValidationFailed
    );
    assert_eq!(
        parse_entries_from_json(r#"{"other":[]}"#).unwrap_err().kind,
        ErrorKind::ValidationFailed
    );

    let uniq = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let missing = std::env::temp_dir().join(format!("d-missing-{uniq}.json"));
    assert_eq!(
        load_entries_from_json_file(&missing).unwrap_err().kind,
        ErrorKind::ValidationFailed
    );
}

#[tokio::test]
async fn runtime_control_readiness_ok_tracks_healthy_fibers() {
    let spec = AssemblySpec {
        resolver: Arc::new(
            StaticResolver::new().register("dummy-ok", DummyOkPlugin),
        ),
        entries: parse_entries_from_json(r#"[{"id":"dummy-ok"}]"#).unwrap(),
    };
    let app = build_from_spec(spec, false).await.expect("build");
    let ctl = app.context().get::<RuntimeControlService>().expect("ctl");
    assert!(ctl.readiness_ok());

    let bad = app
        .context()
        .inject("failing", Deps::empty(), |_| {
            Box::pin(async {
                Err(FrameError::new(ErrorKind::ActivationFailed, "boom"))
            })
        })
        .unwrap();
    let _ = bad.wait().await;
    assert!(!ctl.readiness_ok());

    app.dispose().await.expect("dispose");
}
