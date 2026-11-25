use d_daemon::{AssemblySpec, build_from_spec_named};
use d_mcp::json::{JsonValue, parse};
use d_mcp::transport::{bind_http_listener, serve_http_loop, serve_stdio};
use d_mcp::{
    BoundMcpInfo, McpConfig, McpPlugin, McpServer, McpServerPolicy,
    McpTransportMode,
};
use frame::{
    BoxFuture, ConfigFieldSchema, ConfigFieldType, ConfigValue, Context,
    EntryTarget, ErrorKind, FrameError, LoaderService, Plugin, PluginEntry,
    PluginSchema, StaticResolver,
};
use http_api::{
    HttpHandler, HttpRequest, HttpResponse, RouteMatch, RouteSpec, Router,
    RouterService, StatusCode,
};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::watch;

struct MockRouterHandler;
impl HttpHandler for MockRouterHandler {
    fn call(&self, _req: HttpRequest) -> BoxFuture<'static, HttpResponse> {
        Box::pin(async { HttpResponse::text(StatusCode::OK, "mock-ok") })
    }
}

struct MockRouterPlugin;
impl Plugin for MockRouterPlugin {
    fn name(&self) -> &str {
        "mock-router"
    }

    fn schema(&self) -> PluginSchema {
        PluginSchema::new("Mock HTTP router with sensitive field").field(
            ConfigFieldSchema::new(
                "api_token",
                ConfigFieldType::Str,
                "Secret token that must be redacted in MCP inspection",
            )
            .nullable()
            .sensitive(),
        )
    }

    fn apply(
        &self,
        ctx: Context,
        _config: ConfigValue,
    ) -> BoxFuture<'static, Result<(), FrameError>> {
        Box::pin(async move {
            let router = Arc::new(MockHttpRouter);
            let _ = router.mount_route(
                &ctx,
                RouteSpec {
                    id: "mock".to_string(),
                    host: None,
                    matcher: RouteMatch::CatchAll,
                    methods: None,
                    priority: 0,
                },
                Arc::new(MockRouterHandler),
            )?;
            ctx.provide(Arc::new(RouterService::new(router))).await?;
            Ok(())
        })
    }
}

struct MockHttpRouter;
impl Router for MockHttpRouter {
    fn mount_route(
        &self,
        caller: &Context,
        _spec: RouteSpec,
        _handler: Arc<dyn HttpHandler>,
    ) -> Result<frame::EffectHandle, FrameError> {
        caller.effect_sync("mock-route", |_| Ok(()))
    }

    fn mount_filter(
        &self,
        caller: &Context,
        _filter: Arc<dyn http_api::HttpFilter>,
    ) -> Result<frame::EffectHandle, FrameError> {
        caller.effect_sync("mock-filter", |_| Ok(()))
    }

    fn dispatch(&self, _req: HttpRequest) -> BoxFuture<'static, HttpResponse> {
        Box::pin(async { HttpResponse::text(StatusCode::OK, "mock-ok") })
    }
}

async fn setup_mock_app() -> d_daemon::AppHandle {
    let resolver = Arc::new(
        StaticResolver::new().register("mock-router", MockRouterPlugin),
    );
    let entries = vec![PluginEntry {
        id: "mock-router".to_string(),
        target: EntryTarget::Name("mock-router".to_string()),
        config: ConfigValue::Map(BTreeMap::from([(
            "api_token".to_string(),
            ConfigValue::Str("super-secret-value".to_string()),
        )])),
        disabled: false,
        isolate: Vec::new(),
        children: Vec::new(),
    }];
    build_from_spec_named(
        "mock-assembly",
        AssemblySpec { resolver, entries },
        false,
    )
    .await
    .unwrap()
}

fn make_server(
    app: &d_daemon::AppHandle,
    read_only: bool,
    allow_shutdown: bool,
) -> McpServer {
    let loader = app
        .context()
        .get::<LoaderService>()
        .unwrap()
        .as_ref()
        .clone();
    McpServer::new(
        app.context().clone(),
        loader,
        Arc::clone(app.control()),
        McpServerPolicy {
            read_only,
            allow_shutdown,
            stdio: true,
            http_addr: None,
            http_path: "/mcp".to_string(),
            auth_token_configured: true,
        },
    )
}

#[tokio::test]
async fn protocol_discovery_redaction_and_read_only_policy() {
    let app = setup_mock_app().await;
    let ro_srv = make_server(&app, true, false);

    // Initialize + ping + error cases.
    let init_raw = ro_srv
        .handle_str(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26"}}"#,
        )
        .await
        .unwrap();
    let init_json = parse(&init_raw).unwrap();
    assert_eq!(
        init_json
            .get("result")
            .and_then(|r| r.get("serverInfo"))
            .and_then(|s| s.get("name"))
            .and_then(JsonValue::as_str),
        Some("d-mcp")
    );

    // Notification returns None.
    assert!(
        ro_srv
            .handle_str(
                r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#
            )
            .await
            .is_none()
    );

    // Parse error & unknown method.
    let parse_err = parse(&ro_srv.handle_str("{bad").await.unwrap()).unwrap();
    assert_eq!(
        parse_err
            .get("error")
            .and_then(|e| e.get("code"))
            .and_then(JsonValue::as_i64),
        Some(-32700)
    );

    // Read-only lists only the 5 read-only tools.
    let tools_raw = ro_srv
        .handle_str(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#)
        .await
        .unwrap();
    let tools_json = parse(&tools_raw).unwrap();
    let tool_names: Vec<&str> = tools_json
        .get("result")
        .and_then(|r| r.get("tools"))
        .and_then(JsonValue::as_array)
        .unwrap()
        .iter()
        .filter_map(|t| t.get("name").and_then(JsonValue::as_str))
        .collect();
    assert_eq!(
        tool_names,
        vec![
            "d_server_info",
            "d_catalog",
            "d_inspect",
            "d_validate_patches",
            "d_http_probe",
        ]
    );

    // Calling mutating tool while read_only returns structured READ_ONLY tool error.
    let denied_raw = ro_srv
        .handle_str(
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"d_set_log_level","arguments":{"level":"debug"}}}"#,
        )
        .await
        .unwrap();
    let denied = parse(&denied_raw).unwrap();
    assert_eq!(
        denied
            .get("result")
            .and_then(|r| r.get("isError"))
            .and_then(JsonValue::as_bool),
        Some(true)
    );
    assert!(denied_raw.contains("READ_ONLY"));

    // Sensitive config fields are redacted in d_inspect and d://entries.
    let inspect_raw = ro_srv
        .handle_str(
            r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"d_inspect"}}"#,
        )
        .await
        .unwrap();
    assert!(!inspect_raw.contains("super-secret-value"));
    assert!(inspect_raw.contains("***REDACTED***"));

    let entries_res = ro_srv
        .handle_str(
            r#"{"jsonrpc":"2.0","id":5,"method":"resources/read","params":{"uri":"d://entries"}}"#,
        )
        .await
        .unwrap();
    assert!(!entries_res.contains("super-secret-value"));
    assert!(entries_res.contains("***REDACTED***"));

    // Shutdown policy: blocked unless allow_shutdown=true.
    let mut_srv = make_server(&app, false, false);
    let shut_denied = mut_srv
        .handle_str(
            r#"{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"d_shutdown"}}"#,
        )
        .await
        .unwrap();
    assert!(shut_denied.contains("FORBIDDEN"));
    assert!(!app.control().shutdown_requested());

    let shut_srv = make_server(&app, false, true);
    let shut_ok = shut_srv
        .handle_str(
            r#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"d_shutdown"}}"#,
        )
        .await
        .unwrap();
    assert!(shut_ok.contains("shutdown_requested"));
    assert!(app.control().shutdown_requested());

    app.dispose().await.unwrap();
}

#[tokio::test]
async fn live_http_transport_enforces_auth_methods_and_dispatches_json_rpc() {
    let app = setup_mock_app().await;
    let srv = make_server(&app, false, false);
    let (listener, addr) = bind_http_listener("127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let (stop_tx, stop_rx) = watch::channel(false);
    let task = tokio::spawn(serve_http_loop(
        srv,
        listener,
        "/mcp".to_string(),
        Some(Arc::new("secret-token".to_string())),
        stop_rx,
    ));

    async fn raw_http(addr: std::net::SocketAddr, req: &str) -> String {
        let mut s = TcpStream::connect(addr).await.unwrap();
        s.write_all(req.as_bytes()).await.unwrap();
        let mut buf = Vec::new();
        s.read_to_end(&mut buf).await.unwrap();
        String::from_utf8(buf).unwrap()
    }

    // 401 when Authorization missing or wrong.
    let unauth = raw_http(
        addr,
        "POST /mcp HTTP/1.1\r\nHost: localhost\r\nContent-Length: 2\r\n\r\n{}",
    )
    .await;
    assert!(unauth.starts_with("HTTP/1.1 401"), "{unauth}");

    let wrong_tok = raw_http(
        addr,
        "POST /mcp HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer wrong\r\nContent-Length: 2\r\n\r\n{}",
    )
    .await;
    assert!(wrong_tok.starts_with("HTTP/1.1 401"), "{wrong_tok}");

    // 404 wrong path, 405 wrong method.
    let not_found = raw_http(
        addr,
        "POST /other HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer secret-token\r\nContent-Length: 2\r\n\r\n{}",
    )
    .await;
    assert!(not_found.starts_with("HTTP/1.1 404"), "{not_found}");

    let method_err = raw_http(
        addr,
        "GET /mcp HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer secret-token\r\n\r\n",
    )
    .await;
    assert!(method_err.starts_with("HTTP/1.1 405"), "{method_err}");

    // 202 for notification, 200 for RPC request.
    let notif_body =
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
    let notif_req = format!(
        "POST /mcp HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer secret-token\r\nContent-Length: {}\r\n\r\n{notif_body}",
        notif_body.len()
    );
    let notif_resp = raw_http(addr, &notif_req).await;
    assert!(notif_resp.starts_with("HTTP/1.1 202"), "{notif_resp}");

    let rpc_body = r#"{"jsonrpc":"2.0","id":11,"method":"tools/call","params":{"name":"d_catalog"}}"#;
    let rpc_req = format!(
        "POST /mcp HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer secret-token\r\nContent-Length: {}\r\n\r\n{rpc_body}",
        rpc_body.len()
    );
    let rpc_resp = raw_http(addr, &rpc_req).await;
    assert!(rpc_resp.starts_with("HTTP/1.1 200"), "{rpc_resp}");
    assert!(rpc_resp.contains("mock-router"), "{rpc_resp}");
    assert!(rpc_resp.contains("api_token"), "{rpc_resp}");

    let _ = stop_tx.send(true);
    let _ = task.await;
    app.dispose().await.unwrap();
}

#[tokio::test]
async fn live_stdio_transport_replies_and_triggers_eof_callback() {
    let app = setup_mock_app().await;
    let srv = make_server(&app, false, true);

    let (client_in, server_in) = tokio::io::duplex(4096);
    let (server_out, mut client_out) = tokio::io::duplex(4096);
    let (_stop_tx, stop_rx) = watch::channel(false);
    let eof_called = Arc::new(AtomicBool::new(false));
    let eof_flag = Arc::clone(&eof_called);

    let task = tokio::spawn(serve_stdio(
        srv,
        BufReader::new(server_in),
        server_out,
        stop_rx,
        move || {
            eof_flag.store(true, Ordering::SeqCst);
        },
    ));

    let mut client_writer = client_in;
    client_writer
        .write_all(
            b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n\
              {\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"d_server_info\"}}\n",
        )
        .await
        .unwrap();
    drop(client_writer);

    let mut out_buf = String::new();
    client_out.read_to_string(&mut out_buf).await.unwrap();
    let lines: Vec<&str> = out_buf.lines().collect();
    assert_eq!(lines.len(), 2, "got stdout lines: {out_buf}");
    assert!(lines[0].contains("\"id\":1"));
    assert!(lines[1].contains("mock-assembly"));

    task.await.unwrap();
    assert!(eof_called.load(Ordering::SeqCst));
    app.dispose().await.unwrap();
}

#[tokio::test]
async fn mcp_plugin_validates_config_and_exposes_bound_service() {
    let app = setup_mock_app().await;
    let bad = McpPlugin::default().validate_config(&ConfigValue::Map(
        BTreeMap::from([("transport".into(), ConfigValue::Str("http".into()))]),
    ));
    assert_eq!(bad.unwrap_err().kind, ErrorKind::ValidationFailed);

    let fiber = app
        .context()
        .plugin(
            McpPlugin::new(McpConfig {
                transport: McpTransportMode::Http,
                http_addr: Some("127.0.0.1:0".parse().unwrap()),
                http_path: "/custom-mcp".to_string(),
                auth_token: Some("tok".to_string()),
                read_only: false,
                allow_shutdown: false,
                shutdown_on_eof: false,
            }),
            ConfigValue::Null,
        )
        .unwrap();
    fiber.wait().await.unwrap();

    let bound = app.context().get::<BoundMcpInfo>().unwrap();
    assert!(bound.http_addr.is_some());
    assert_eq!(bound.http_path, "/custom-mcp");

    app.dispose().await.unwrap();
}
