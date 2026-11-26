#![cfg(feature = "mcp")]

use d::{Options, build_runtime};
use d_mcp::json::{JsonValue, parse};
use d_mcp::{McpServer, McpServerPolicy};
use frame::LoaderService;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

fn server_for(app: &d::AppHandle) -> McpServer {
    let loader = app
        .context()
        .get::<LoaderService>()
        .expect("LoaderService bound by d-daemon")
        .as_ref()
        .clone();
    McpServer::new(
        app.context().clone(),
        loader,
        Arc::clone(app.control()),
        McpServerPolicy {
            read_only: false,
            allow_shutdown: true,
            stdio: false,
            http_addr: None,
            http_path: "/mcp".to_string(),
            auth_token_configured: false,
        },
    )
}

async fn call_tool(
    srv: &McpServer,
    id: i64,
    name: &str,
    args: &str,
) -> JsonValue {
    let payload = format!(
        r#"{{"jsonrpc":"2.0","id":{id},"method":"tools/call","params":{{"name":"{name}","arguments":{args}}}}}"#
    );
    let raw = srv.handle_str(&payload).await.unwrap();
    let parsed = parse(&raw).unwrap();
    parsed
        .get("result")
        .cloned()
        .expect("expected JSON-RPC result")
}

#[tokio::test]
async fn mcp_discovers_all_plugins_and_dynamically_reconfigures_everything() {
    let dir1 = TempDir::new().unwrap();
    let root1 = dir1.path().canonicalize().unwrap();
    std::fs::write(root1.join("first.txt"), "from-first-root").unwrap();
    std::fs::write(root1.join(".dotfile"), "hidden-secret").unwrap();
    std::fs::write(
        root1.join("big.txt"),
        "compressible-mcp-payload-".repeat(20),
    )
    .unwrap();

    let dir2 = TempDir::new().unwrap();
    let root2 = dir2.path().canonicalize().unwrap();
    std::fs::write(root2.join("second.bin"), b"from-second-root").unwrap();
    std::fs::write(root2.join(".dot2"), "hidden-two").unwrap();

    let app = build_runtime(
        root1.clone(),
        Options {
            allow_hidden: false,
            cors: false,
        },
        None,
    )
    .await
    .unwrap();
    let srv = server_for(&app);

    // 1. Live schema discovery returns all 9 full-assembly plugins + descriptions + fields.
    let catalog = call_tool(&srv, 1, "d_catalog", "{}").await;
    assert_eq!(
        catalog.get("isError").and_then(JsonValue::as_bool),
        Some(false)
    );
    let plugins = catalog
        .get("structuredContent")
        .and_then(|s| s.get("plugins"))
        .and_then(JsonValue::as_array)
        .unwrap();
    let names: Vec<&str> = plugins
        .iter()
        .filter_map(|p| p.get("name").and_then(JsonValue::as_str))
        .collect();
    for expected in [
        "app-files",
        "doc-render",
        "ext-compression",
        "ext-cors",
        "l4-transport",
        "proto-http",
        "viewer-code",
        "viewer-markdown",
        "viewer-media",
    ] {
        assert!(names.contains(&expected), "missing {expected} in {names:?}");
    }

    // 2. Toggle ext-cors ON with custom allow_origin and verify via d_http_probe.
    let cors_on = call_tool(
        &srv,
        2,
        "d_toggle_plugin",
        r#"{"id":"ext-cors","enabled":true,"config":{"allow_origin":"https://app.example"}}"#,
    )
    .await;
    assert_eq!(
        cors_on.get("isError").and_then(JsonValue::as_bool),
        Some(false)
    );

    let probe_cors = call_tool(
        &srv,
        3,
        "d_http_probe",
        r#"{"uri":"/first.txt?view=raw","headers":{"Origin":"https://app.example"}}"#,
    )
    .await;
    let cors_headers = probe_cors
        .get("structuredContent")
        .and_then(|s| s.get("headers"))
        .and_then(JsonValue::as_array)
        .unwrap();
    assert!(cors_headers.iter().any(|h| {
        h.get("name").and_then(JsonValue::as_str)
            == Some("Access-Control-Allow-Origin")
            && h.get("value").and_then(JsonValue::as_str)
                == Some("https://app.example")
    }));

    // Disable ext-cors cleanly.
    let cors_off = call_tool(
        &srv,
        4,
        "d_toggle_plugin",
        r#"{"id":"ext-cors","enabled":false,"remove":true}"#,
    )
    .await;
    assert_eq!(
        cors_off.get("isError").and_then(JsonValue::as_bool),
        Some(false)
    );

    // 3. Dynamically tune compression threshold min_size.
    let comp_high = call_tool(
        &srv,
        5,
        "d_set_plugin_config",
        r#"{"id":"ext-compression","config":{"min_size":10000}}"#,
    )
    .await;
    assert_eq!(
        comp_high.get("isError").and_then(JsonValue::as_bool),
        Some(false)
    );
    let probe_uncompressed = call_tool(
        &srv,
        6,
        "d_http_probe",
        r#"{"uri":"/big.txt?view=raw","headers":{"Accept-Encoding":"br, gzip"}}"#,
    )
    .await;
    let unc_headers = probe_uncompressed
        .get("structuredContent")
        .and_then(|s| s.get("headers"))
        .and_then(JsonValue::as_array)
        .unwrap();
    assert!(!unc_headers.iter().any(|h| {
        h.get("name")
            .and_then(JsonValue::as_str)
            .is_some_and(|n| n.eq_ignore_ascii_case("content-encoding"))
    }));

    let comp_low = call_tool(
        &srv,
        7,
        "d_set_plugin_config",
        r#"{"id":"ext-compression","config":{"min_size":10}}"#,
    )
    .await;
    assert_eq!(
        comp_low.get("isError").and_then(JsonValue::as_bool),
        Some(false)
    );
    let probe_compressed = call_tool(
        &srv,
        8,
        "d_http_probe",
        r#"{"uri":"/big.txt?view=raw","headers":{"Accept-Encoding":"br, gzip"}}"#,
    )
    .await;
    let cmp_headers = probe_compressed
        .get("structuredContent")
        .and_then(|s| s.get("headers"))
        .and_then(JsonValue::as_array)
        .unwrap();
    assert!(cmp_headers.iter().any(|h| {
        h.get("name")
            .and_then(JsonValue::as_str)
            .is_some_and(|n| n.eq_ignore_ascii_case("content-encoding"))
    }));

    // 4. Dry-run invalid root fails cleanly; valid switch to root2 + allow_hidden=true succeeds.
    let bad_root = call_tool(
        &srv,
        9,
        "d_validate_patches",
        r#"{"patches":[{"op":"replace_config","id":"app-files","config":{"root":"/definitely/missing/d-root-dir"}}]}"#,
    )
    .await;
    assert_eq!(
        bad_root.get("isError").and_then(JsonValue::as_bool),
        Some(true)
    );

    let switch_root = call_tool(
        &srv,
        10,
        "d_set_plugin_config",
        &format!(
            r#"{{"id":"app-files","config":{{"root":{},"allow_hidden":true}}}}"#,
            JsonValue::Str(root2.to_string_lossy().into_owned())
                .to_compact_string()
        ),
    )
    .await;
    assert_eq!(
        switch_root.get("isError").and_then(JsonValue::as_bool),
        Some(false)
    );
    let probe_root2 =
        call_tool(&srv, 11, "d_http_probe", r#"{"uri":"/"}"#).await;
    let listing = probe_root2
        .get("structuredContent")
        .and_then(|s| s.get("body_preview"))
        .and_then(JsonValue::as_str)
        .unwrap();
    assert!(listing.contains("second.bin"));
    assert!(listing.contains(".dot2"));

    // 5. Tune l4-transport drain & dynamically bind proto-http TCP listener from None -> 127.0.0.1:0 -> live TCP -> null.
    let tune_l4 = call_tool(
        &srv,
        12,
        "d_set_plugin_config",
        r#"{"id":"l4-transport","config":{"short_drain_ms":100,"long_drain_ms":150}}"#,
    )
    .await;
    assert_eq!(
        tune_l4.get("isError").and_then(JsonValue::as_bool),
        Some(false)
    );

    let bind_res = call_tool(
        &srv,
        13,
        "d_set_plugin_config",
        r#"{"id":"proto-http","config":{"bind_addr":"127.0.0.1:0","max_headers":64}}"#,
    )
    .await;
    assert_eq!(
        bind_res.get("isError").and_then(JsonValue::as_bool),
        Some(false)
    );
    let addr_str = bind_res
        .get("structuredContent")
        .and_then(|s| s.get("server"))
        .and_then(|s| s.get("local_addr"))
        .and_then(JsonValue::as_str)
        .expect("proto-http dynamically bound to local_addr");
    let bound_addr: std::net::SocketAddr = addr_str.parse().unwrap();

    // Real TCP request hits dynamically bound HTTP server serving switched root2!
    let mut tcp = TcpStream::connect(bound_addr).await.unwrap();
    tcp.write_all(b"GET /second.bin HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut tcp_out = Vec::new();
    tcp.read_to_end(&mut tcp_out).await.unwrap();
    let tcp_text = String::from_utf8(tcp_out).unwrap();
    assert!(tcp_text.starts_with("HTTP/1.1 200"));
    assert!(tcp_text.contains("from-second-root"));

    // Unbind back to in-memory only via bind_addr: null.
    let unbind = call_tool(
        &srv,
        14,
        "d_set_plugin_config",
        r#"{"id":"proto-http","config":{"bind_addr":null}}"#,
    )
    .await;
    assert_eq!(
        unbind
            .get("structuredContent")
            .and_then(|s| s.get("server"))
            .and_then(|s| s.get("local_addr")),
        Some(&JsonValue::Null)
    );

    // 6. Runtime log level adjustment & fiber restart.
    let log_res =
        call_tool(&srv, 15, "d_set_log_level", r#"{"level":"debug"}"#).await;
    assert_eq!(
        log_res
            .get("structuredContent")
            .and_then(|s| s.get("current"))
            .and_then(JsonValue::as_str),
        Some("debug")
    );
    let _ = call_tool(&srv, 16, "d_set_log_level", r#"{"level":"info"}"#).await;

    let restart_res =
        call_tool(&srv, 17, "d_restart_plugin", r#"{"id":"doc-render"}"#).await;
    assert_eq!(
        restart_res.get("isError").and_then(JsonValue::as_bool),
        Some(false)
    );

    app.dispose().await.unwrap();
}
