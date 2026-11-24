use d_minimal::{Options, create_app, create_app_with_options};
use http_api::{HttpRequest, HttpResponse, Method, StatusCode};
use render_api::{MarkdownOptions, RenderService};
use tempfile::TempDir;

async fn body_bytes(resp: HttpResponse) -> Vec<u8> {
    resp.body.into_bytes().await.unwrap()
}

fn get(uri: &str) -> HttpRequest {
    HttpRequest::new(Method::Get, uri)
}

#[tokio::test]
async fn minimal_static_range_etag_301_and_no_compression() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    std::fs::write(root.join("hello.bin"), b"0123456789abcdef").unwrap();
    std::fs::write(root.join("big.txt"), "minimal static body ".repeat(100))
        .unwrap();
    std::fs::create_dir(root.join("sub")).unwrap();
    std::fs::write(root.join("sub/a.bin"), b"ok").unwrap();

    let app = create_app(root, false).await;

    // Static 200 without compression even when Accept-Encoding is sent.
    let resp = app
        .dispatch(
            get("/big.txt?view=raw").with_header("Accept-Encoding", "br, gzip"),
        )
        .await;
    assert_eq!(resp.status, StatusCode::OK);
    assert!(
        resp.header("content-encoding").is_none(),
        "d-minimal must not include compression filters"
    );

    // Directory redirect 301.
    let redir = app.dispatch(get("/sub")).await;
    assert_eq!(redir.status, StatusCode::MOVED_PERMANENTLY);
    assert_eq!(redir.header("location"), Some("/sub/"));

    // Directory listing 200.
    let list = app.dispatch(get("/sub/")).await;
    assert_eq!(list.status, StatusCode::OK);
    let html = String::from_utf8(body_bytes(list).await).unwrap();
    assert!(html.contains("a.bin"));

    // Range 206 & 416 on binary/raw static path.
    let r206 = app
        .dispatch(get("/hello.bin").with_header("Range", "bytes=4-7"))
        .await;
    assert_eq!(r206.status, StatusCode::PARTIAL_CONTENT);
    assert_eq!(&body_bytes(r206).await[..], b"4567");

    let r416 = app
        .dispatch(get("/hello.bin").with_header("Range", "bytes=99-120"))
        .await;
    assert_eq!(r416.status, StatusCode::RANGE_NOT_SATISFIABLE);

    // ETag 304.
    let first = app.dispatch(get("/hello.bin")).await;
    let etag = first.header("etag").unwrap().to_string();
    let r304 = app
        .dispatch(get("/hello.bin").with_header("If-None-Match", etag))
        .await;
    assert_eq!(r304.status, StatusCode::NOT_MODIFIED);

    // PlainRenderer contract swap check.
    let renderer = app
        .context()
        .get::<RenderService>()
        .expect("RenderService must be bound to doc-render-plain");
    let rendered = renderer.render_markdown(
        "# Hi <script>x</script>",
        &MarkdownOptions::default(),
    );
    assert!(rendered.html.contains("<pre class=\"d-plain-doc\">"));
    assert!(rendered.html.contains("&lt;script&gt;"));
    assert!(!rendered.html.contains("<script>"));

    app.dispose().await.unwrap();
}

#[tokio::test]
async fn minimal_supports_cors_option() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("x.txt"), b"x").unwrap();

    let app = create_app_with_options(
        dir.path(),
        Options {
            allow_hidden: false,
            cors: true,
        },
    )
    .await;

    let resp = app
        .dispatch(get("/x.txt").with_header("Origin", "https://example.org"))
        .await;
    assert_eq!(resp.status, StatusCode::OK);
    assert_eq!(resp.header("access-control-allow-origin"), Some("*"));

    app.dispose().await.unwrap();
}
