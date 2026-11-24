//! BDD/TDD tests for Layer 4 `app-files`: trailing-slash 301, sandbox,
//! Range/ETag/304/HEAD/RFC5987, index.html, sort/hidden, and white listing UI
//! (`T-H08`, `T-H09`, `T-H10`, `A22`).

use app_files::{FilesAppPlugin, FilesConfig, mime_for_path};
use frame::{BoxFuture, ConfigValue, Context, FrameOptions, Spawner};
use http_api::{HttpRequest, Method, RouterService, StatusCode};
use proto_http::HttpEnginePlugin;
use std::path::Path;
use std::sync::Arc;
use tempfile::TempDir;

#[derive(Default)]
struct TokioTestSpawner;

impl Spawner for TokioTestSpawner {
    fn spawn(&self, fut: BoxFuture<'static, ()>) {
        tokio::spawn(fut);
    }
}

async fn setup_files(allow_hidden: bool) -> (Context, RouterService, TempDir) {
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path();
    std::fs::write(root.join("hello.bin"), (0u8..16).collect::<Vec<u8>>())
        .unwrap();
    std::fs::write(root.join("code.rs"), "fn main() {}\n").unwrap();
    std::fs::write(root.join("big.bin"), vec![b'X'; 512]).unwrap();
    std::fs::write(root.join("中文.txt"), "你好，世界").unwrap();
    std::fs::write(root.join(".hidden"), "secret").unwrap();
    std::fs::create_dir(root.join("subdir")).unwrap();
    std::fs::write(root.join("subdir/nested.txt"), "nested").unwrap();

    let canonical = root.canonicalize().unwrap();
    let ctx = Context::new(FrameOptions::new(Arc::new(TokioTestSpawner)));
    ctx.plugin(HttpEnginePlugin::default(), ConfigValue::Null)
        .expect("http engine");
    ctx.plugin(
        FilesAppPlugin::new(FilesConfig::new(canonical, allow_hidden)),
        ConfigValue::Null,
    )
    .expect("app-files");
    ctx.settle().await;
    let router = ctx.get::<RouterService>().expect("router").as_ref().clone();
    (ctx, router, dir)
}

#[tokio::test]
async fn built_in_mime_and_trailing_slash_canonicalization() {
    assert_eq!(mime_for_path(Path::new("diagram.svg")), "image/svg+xml");
    assert_eq!(
        mime_for_path(Path::new("app.js")),
        "text/javascript; charset=utf-8"
    );
    assert_eq!(
        mime_for_path(Path::new("style.css")),
        "text/css; charset=utf-8"
    );
    assert_eq!(
        mime_for_path(Path::new("data.bin")),
        "application/octet-stream"
    );

    let (ctx, router, _dir) = setup_files(false).await;

    // Bare directory without trailing slash returns 301 preserving query string.
    let redir = router
        .dispatch(HttpRequest::new(Method::Get, "/subdir"))
        .await;
    assert_eq!(redir.status, StatusCode::MOVED_PERMANENTLY);
    assert_eq!(redir.header("location"), Some("/subdir/"));

    let redir_q = router
        .dispatch(HttpRequest::new(Method::Get, "/subdir?sort=size"))
        .await;
    assert_eq!(redir_q.status, StatusCode::MOVED_PERMANENTLY);
    assert_eq!(redir_q.header("location"), Some("/subdir/?sort=size"));

    let ok = router
        .dispatch(HttpRequest::new(Method::Get, "/subdir/"))
        .await;
    assert_eq!(ok.status, StatusCode::OK);
    let html = String::from_utf8(ok.body.into_bytes().await.unwrap()).unwrap();
    assert!(html.contains("nested.txt"));
    assert!(html.contains("#ffffff"));
    assert!(!html.contains("#0d1117"));
    assert!(!html.contains("📁"));

    ctx.dispose().await.expect("dispose");
}

#[tokio::test]
async fn sandbox_range_etag_ims_head_and_download() {
    let (ctx, router, dir) = setup_files(false).await;

    assert_eq!(
        router
            .dispatch(HttpRequest::new(Method::Get, "/favicon.ico"))
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        router
            .dispatch(HttpRequest::new(Method::Get, "/..%2f..%2fetc%2fpasswd"))
            .await
            .status,
        StatusCode::NOT_FOUND
    );

    #[cfg(unix)]
    {
        std::os::unix::fs::symlink("/etc/hosts", dir.path().join("escape"))
            .unwrap();
        assert_eq!(
            router
                .dispatch(HttpRequest::new(Method::Get, "/escape"))
                .await
                .status,
            StatusCode::NOT_FOUND
        );
    }

    // HEAD preserves Content-Length, empty body
    let head = router
        .dispatch(HttpRequest::new(Method::Head, "/hello.bin"))
        .await;
    assert_eq!(head.status, StatusCode::OK);
    assert_eq!(head.header("content-length"), Some("16"));
    assert!(head.body.into_bytes().await.unwrap().is_empty());

    // Range slice + 416
    let partial = router
        .dispatch(
            HttpRequest::new(Method::Get, "/hello.bin")
                .with_header("Range", "bytes=5-9"),
        )
        .await;
    assert_eq!(partial.status, StatusCode::PARTIAL_CONTENT);
    assert_eq!(partial.header("content-range"), Some("bytes 5-9/16"));
    assert_eq!(
        partial.body.into_bytes().await.unwrap(),
        vec![5, 6, 7, 8, 9]
    );

    let oob = router
        .dispatch(
            HttpRequest::new(Method::Get, "/hello.bin")
                .with_header("Range", "bytes=999-1000"),
        )
        .await;
    assert_eq!(oob.status, StatusCode::RANGE_NOT_SATISFIABLE);

    // ETag 304
    let first = router
        .dispatch(HttpRequest::new(Method::Get, "/hello.bin"))
        .await;
    let etag = first.header("etag").expect("etag").to_string();
    let not_mod = router
        .dispatch(
            HttpRequest::new(Method::Get, "/hello.bin")
                .with_header("If-None-Match", etag),
        )
        .await;
    assert_eq!(not_mod.status, StatusCode::NOT_MODIFIED);

    // RFC 5987 download
    let dl = router
        .dispatch(HttpRequest::new(
            Method::Get,
            "/%E4%B8%AD%E6%96%87.txt?view=download",
        ))
        .await;
    assert_eq!(dl.status, StatusCode::OK);
    assert!(
        dl.header("content-disposition")
            .unwrap()
            .contains("filename*=UTF-8''%E4%B8%AD%E6%96%87.txt")
    );

    ctx.dispose().await.expect("dispose");
}

#[tokio::test]
async fn index_html_listing_sort_and_hidden_toggle() {
    let (ctx, router, dir) = setup_files(true).await;

    // Hidden visible by default when allow_hidden=true, toggleable off
    let root_html = String::from_utf8(
        router
            .dispatch(HttpRequest::new(Method::Get, "/"))
            .await
            .body
            .into_bytes()
            .await
            .unwrap(),
    )
    .unwrap();
    assert!(root_html.contains(".hidden"));

    let no_hidden = String::from_utf8(
        router
            .dispatch(HttpRequest::new(Method::Get, "/?hidden=false"))
            .await
            .body
            .into_bytes()
            .await
            .unwrap(),
    )
    .unwrap();
    assert!(!no_hidden.contains(".hidden"));

    // Sort by size descending/ascending check: big.bin (512B) vs hello.bin (16B)
    let sorted = String::from_utf8(
        router
            .dispatch(HttpRequest::new(Method::Get, "/?sort=size"))
            .await
            .body
            .into_bytes()
            .await
            .unwrap(),
    )
    .unwrap();
    let big_pos = sorted.find("big.bin").expect("big.bin");
    let hello_pos = sorted.find("hello.bin").expect("hello.bin");
    assert!(
        hello_pos < big_pos,
        "expected size sort ascending: {sorted}"
    );

    // index.html & ?listing=true
    std::fs::write(dir.path().join("subdir/index.html"), "<h1>home</h1>")
        .unwrap();
    let idx = router
        .dispatch(HttpRequest::new(Method::Get, "/subdir/"))
        .await;
    assert_eq!(idx.body.into_bytes().await.unwrap(), b"<h1>home</h1>");

    let bypass = router
        .dispatch(HttpRequest::new(Method::Get, "/subdir/?listing=true"))
        .await;
    let bypass_html =
        String::from_utf8(bypass.body.into_bytes().await.unwrap()).unwrap();
    assert!(bypass_html.contains("Index of"));
    assert!(bypass_html.contains("nested.txt"));

    ctx.dispose().await.expect("dispose");
}
