//! End-to-end integration tests for the full `d` gateway assembly (`GA23`–`GA24`):
//! - `-c gateway.json` manifest boot + `SIGHUP` preflight rejection & incremental
//!   reload keeping unrelated long-lived SSE streams alive;
//! - Full 17-plugin stack + TLS/Proxy/WS/Auth/RateLimit/Observability + `d_catalog`.

use d::{Options, build_spec};
use d_daemon::{build_from_spec, parse_entries_from_json};
use http_api::{HttpRequest, Method, StatusCode};
#[cfg(unix)]
use std::io::Write;
#[cfg(unix)]
use std::net::{SocketAddr, TcpListener};
#[cfg(unix)]
use std::process::{Child, Command, Stdio};
#[cfg(unix)]
use std::time::{Duration, Instant};
use tempfile::TempDir;
#[cfg(unix)]
use tokio::io::{AsyncReadExt, AsyncWriteExt};
#[cfg(unix)]
use tokio::net::TcpStream;

#[cfg(unix)]
struct ChildGuard(Option<Child>);

#[cfg(unix)]
impl ChildGuard {
    fn pid(&self) -> u32 {
        self.0.as_ref().expect("child").id()
    }
}

#[cfg(unix)]
impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(mut c) = self.0.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

#[cfg(unix)]
fn free_loopback_addr() -> SocketAddr {
    let l = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    l.local_addr().unwrap()
}

#[cfg(unix)]
fn write_json(path: &std::path::Path, content: &str) {
    let mut f = std::fs::File::create(path).expect("create manifest");
    f.write_all(content.as_bytes()).expect("write manifest");
    f.sync_all().expect("sync manifest");
}

#[cfg(unix)]
fn send_sighup(pid: u32) {
    let status = Command::new("kill")
        .args(["-HUP", &pid.to_string()])
        .status()
        .expect("spawn kill -HUP");
    assert!(status.success(), "kill -HUP failed: {status}");
}

#[cfg(unix)]
#[tokio::test]
async fn sighup_reload_keeps_unrelated_sse_stream_alive_and_rejects_bad_manifest()
 {
    let dir = TempDir::new().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let www = root.join("www");
    std::fs::create_dir(&www).unwrap();
    std::fs::write(www.join("hello.txt"), "hello-v1").unwrap();

    let addr = free_loopback_addr();
    let manifest_path = root.join("gateway.json");
    let make_manifest = |with_cors: bool| {
        let cors_block = if with_cors {
            r#",{"id":"ext-cors","config":{"allow_origin":"https://reloaded.example"}}"#
        } else {
            ""
        };
        format!(
            r#"{{
  "plugins": [
    {{"id":"l4-transport","config":{{"short_drain_ms":40,"long_drain_ms":100}}}},
    {{"id":"proto-http","config":{{"bind_addr":"{addr}"}}}},
    {{"id":"proto-ws"}},
    {{"id":"app-files","config":{{"root":"{}"}}}},
    {{"id":"ext-dev-kit","config":{{
      "inspect_enabled": false,
      "mocks": [{{
        "id": "sse-long",
        "method": "GET",
        "path": "/sse",
        "status": 200,
        "sse_chunks": [
          {{"data": "data: first\n\n", "delay_ms": 0}},
          {{"data": "data: after-sighup\n\n", "delay_ms": 700}}
        ]
      }}]
    }}}}{cors_block}
  ]
}}"#,
            www.display()
        )
    };
    write_json(&manifest_path, &make_manifest(false));

    let child = Command::new(env!("CARGO_BIN_EXE_d"))
        .args(["-c", &manifest_path.to_string_lossy()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn d -c");
    let guard = ChildGuard(Some(child));

    // Wait for daemon TCP port to come up, then open long-lived SSE stream.
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut sse_stream = loop {
        match TcpStream::connect(addr).await {
            Ok(s) => break s,
            Err(_) if Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            Err(e) => panic!("daemon failed to listen on {addr}: {e}"),
        }
    };
    sse_stream
        .write_all(b"GET /sse HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();

    let mut sse_buf = Vec::new();
    let mut tmp = [0u8; 512];
    while !String::from_utf8_lossy(&sse_buf).contains("data: first") {
        let n = tokio::time::timeout(
            Duration::from_secs(3),
            sse_stream.read(&mut tmp),
        )
        .await
        .expect("sse first chunk timeout")
        .unwrap();
        assert!(n > 0, "sse closed prematurely before first chunk");
        sse_buf.extend_from_slice(&tmp[..n]);
    }

    // 1. Invalid manifest on SIGHUP is rejected without disturbing service.
    write_json(
        &manifest_path,
        r#"{"plugins":[{"id":"proto-http","config":{"bind_addr":12345}}]}"#,
    );
    send_sighup(guard.pid());
    tokio::time::sleep(Duration::from_millis(80)).await;

    let mut check_conn = TcpStream::connect(addr).await.unwrap();
    check_conn
        .write_all(
            b"GET /hello.txt?view=raw HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        )
        .await
        .unwrap();
    let mut check_out = Vec::new();
    check_conn.read_to_end(&mut check_out).await.unwrap();
    let check_txt = String::from_utf8_lossy(&check_out);
    assert!(
        check_txt.contains("200 OK") && check_txt.contains("hello-v1"),
        "{check_txt}"
    );

    // 2. Valid manifest adding ext-cors applies incrementally on SIGHUP.
    write_json(&manifest_path, &make_manifest(true));
    send_sighup(guard.pid());

    let reload_deadline = Instant::now() + Duration::from_secs(4);
    loop {
        let mut c = TcpStream::connect(addr).await.unwrap();
        c.write_all(
            b"GET /hello.txt?view=raw HTTP/1.1\r\nHost: localhost\r\nOrigin: https://reloaded.example\r\nConnection: close\r\n\r\n",
        )
        .await
        .unwrap();
        let mut buf = Vec::new();
        c.read_to_end(&mut buf).await.unwrap();
        let txt = String::from_utf8_lossy(&buf);
        if txt.contains("Access-Control-Allow-Origin: https://reloaded.example")
        {
            break;
        }
        if Instant::now() >= reload_deadline {
            panic!(
                "CORS header never appeared after SIGHUP reload; last={txt}"
            );
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
    }

    // 3. The unrelated pre-SIGHUP SSE stream receives the delayed second chunk intact!
    let mut rest = Vec::new();
    tokio::time::timeout(
        Duration::from_secs(4),
        sse_stream.read_to_end(&mut rest),
    )
    .await
    .expect("sse completion timeout")
    .unwrap();
    sse_buf.extend_from_slice(&rest);
    let full_sse = String::from_utf8_lossy(&sse_buf);
    assert!(
        full_sse.contains("data: after-sighup"),
        "expected SSE stream to survive unrelated SIGHUP reload: {full_sse}"
    );
}

#[tokio::test]
async fn full_gateway_stack_and_mcp_discover_all_17_plugins() {
    let dir = TempDir::new().unwrap();
    let root = dir.path().canonicalize().unwrap();
    std::fs::write(root.join("index.html"), "<h1>gw</h1>").unwrap();

    let mut spec = build_spec(root.clone(), Options::default(), None);
    let root_json = root.display().to_string().replace('\\', "\\\\");
    let manifest = format!(
        r#"{{
  "plugins": [
    {{"id":"l4-transport"}},
    {{"id":"l4-tls"}},
    {{"id":"proto-http"}},
    {{"id":"proto-ws"}},
    {{"id":"proto-tcp"}},
    {{"id":"ext-proxy"}},
    {{"id":"ext-observability"}},
    {{"id":"ext-ratelimit","config":{{"rules":[{{"prefix":"/limited","rps":100,"burst":1}}]}}}},
    {{"id":"ext-auth","config":{{"rules":[{{"prefix":"/secret","bearer_tokens":["t-1"]}}]}}}},
    {{"id":"ext-dev-kit","config":{{"inspect_enabled":true}}}},
    {{"id":"ext-compression"}},
    {{"id":"ext-cors"}},
    {{"id":"app-files","config":{{"root":"{root_json}"}}}},
    {{"id":"doc-render"}},
    {{"id":"viewer-markdown"}},
    {{"id":"viewer-code"}},
    {{"id":"viewer-media"}}
  ]
}}"#,
    );
    spec.entries = parse_entries_from_json(&manifest).expect("manifest");
    let app = build_from_spec(spec, false)
        .await
        .expect("boot full 17 plugins");

    // Verify governance chain + probes work together in-process.
    assert_eq!(
        app.dispatch(HttpRequest::new(Method::Get, "/healthz"))
            .await
            .status,
        StatusCode::OK
    );
    assert_eq!(
        app.dispatch(HttpRequest::new(Method::Get, "/readyz"))
            .await
            .status,
        StatusCode::OK
    );
    assert_eq!(
        app.dispatch(HttpRequest::new(Method::Get, "/secret/doc"))
            .await
            .status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        app.dispatch(
            HttpRequest::new(Method::Get, "/secret/doc")
                .with_header("Authorization", "Bearer t-1")
        )
        .await
        .status,
        StatusCode::NOT_FOUND
    );
    let _first_ok = app
        .dispatch(HttpRequest::new(Method::Get, "/limited/a"))
        .await;
    assert_eq!(
        app.dispatch(HttpRequest::new(Method::Get, "/limited/a"))
            .await
            .status,
        StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(
        app.dispatch(HttpRequest::new(Method::Get, "/_d/inspect"))
            .await
            .status,
        StatusCode::OK
    );

    // Verify MCP d_catalog self-discovers all 17 plugins & schemas.
    #[cfg(feature = "mcp")]
    {
        use d_mcp::json::{JsonValue, parse};
        use d_mcp::{McpServer, McpServerPolicy};
        use frame::LoaderService;
        use std::sync::Arc;

        let loader = app
            .context()
            .get::<LoaderService>()
            .unwrap()
            .as_ref()
            .clone();
        let srv = McpServer::new(
            app.context().clone(),
            loader,
            Arc::clone(app.control()),
            McpServerPolicy {
                read_only: false,
                allow_shutdown: false,
                stdio: false,
                http_addr: None,
                http_path: "/mcp".to_string(),
                auth_token_configured: false,
            },
        );
        let raw = srv
            .handle_str(r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"d_catalog","arguments":{}}}"#)
            .await
            .unwrap();
        let parsed = parse(&raw).unwrap();
        let plugins = parsed
            .get("result")
            .and_then(|r| r.get("structuredContent"))
            .and_then(|s| s.get("plugins"))
            .and_then(JsonValue::as_array)
            .expect("plugins array");
        let names: Vec<&str> = plugins
            .iter()
            .filter_map(|p| p.get("name").and_then(JsonValue::as_str))
            .collect();
        assert_eq!(names.len(), 17, "expected 17 catalog plugins: {names:?}");
        for expected in [
            "l4-transport",
            "l4-tls",
            "proto-http",
            "proto-ws",
            "proto-tcp",
            "ext-proxy",
            "ext-observability",
            "ext-ratelimit",
            "ext-auth",
            "ext-dev-kit",
            "ext-compression",
            "ext-cors",
            "app-files",
            "doc-render",
            "viewer-markdown",
            "viewer-code",
            "viewer-media",
        ] {
            assert!(
                names.contains(&expected),
                "missing {expected} in {names:?}"
            );
        }
    }

    app.dispose().await.unwrap();
}
