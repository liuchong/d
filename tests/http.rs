//! System integration tests for `d` exercised against the real assembled
//! router pipeline (`AppHandle::dispatch`), plus workspace hygiene gates.

use app_files::format_http_date;
use d::{AppHandle, Options, create_app, create_app_with_options};
use http_api::{HttpRequest, HttpResponse, Method, StatusCode};
use std::path::Path;
use tempfile::TempDir;

async fn test_app(allow_hidden: bool) -> (AppHandle, TempDir) {
    let dir = TempDir::new().unwrap();
    let root = dir.path();

    std::fs::write(root.join("hello.bin"), (0u8..16).collect::<Vec<u8>>())
        .unwrap();
    std::fs::write(root.join("big.bin"), vec![b'B'; 512]).unwrap();
    std::fs::write(root.join("code.rs"), "fn main() {}\n").unwrap();
    std::fs::write(
        root.join("big.txt"),
        "compressible integration test payload! ".repeat(24),
    )
    .unwrap();
    std::fs::write(root.join("中文.txt"), "你好，世界").unwrap();
    std::fs::write(root.join(".hidden"), "secret").unwrap();
    std::fs::create_dir(root.join("subdir")).unwrap();
    std::fs::write(root.join("subdir/nested.txt"), "nested").unwrap();

    let canonical = root.canonicalize().unwrap();
    (create_app(canonical, allow_hidden).await, dir)
}

fn get(uri: &str) -> HttpRequest {
    HttpRequest::new(Method::Get, uri)
}

async fn body_bytes(response: HttpResponse) -> Vec<u8> {
    response.body.into_bytes().await.unwrap()
}

#[tokio::test]
async fn get_file_ok() {
    let (app, _dir) = test_app(false).await;
    let resp = app.dispatch(get("/hello.bin")).await;

    assert_eq!(resp.status, StatusCode::OK);
    assert_eq!(resp.header("content-length").unwrap(), "16");
    assert_eq!(resp.header("accept-ranges").unwrap(), "bytes");
    assert!(resp.header("etag").is_some());
    assert!(resp.header("last-modified").is_some());

    let body = body_bytes(resp).await;
    assert_eq!(&body[..], &(0u8..16).collect::<Vec<u8>>()[..]);
}

#[tokio::test]
async fn head_file_headers_only() {
    let (app, _dir) = test_app(false).await;
    let resp = app
        .dispatch(HttpRequest::new(Method::Head, "/hello.bin"))
        .await;

    assert_eq!(resp.status, StatusCode::OK);
    assert_eq!(resp.header("content-length").unwrap(), "16");
    assert!(body_bytes(resp).await.is_empty());
}

#[tokio::test]
async fn range_request_returns_slice() {
    let (app, _dir) = test_app(false).await;
    let resp = app
        .dispatch(get("/hello.bin").with_header("Range", "bytes=5-9"))
        .await;

    assert_eq!(resp.status, StatusCode::PARTIAL_CONTENT);
    assert_eq!(resp.header("content-range").unwrap(), "bytes 5-9/16");
    assert_eq!(resp.header("content-length").unwrap(), "5");
    assert_eq!(&body_bytes(resp).await[..], &[5u8, 6, 7, 8, 9]);
}

#[tokio::test]
async fn range_suffix_returns_tail() {
    let (app, _dir) = test_app(false).await;
    let resp = app
        .dispatch(get("/hello.bin").with_header("Range", "bytes=-4"))
        .await;

    assert_eq!(resp.status, StatusCode::PARTIAL_CONTENT);
    assert_eq!(resp.header("content-range").unwrap(), "bytes 12-15/16");
    assert_eq!(&body_bytes(resp).await[..], &[12u8, 13, 14, 15]);
}

#[tokio::test]
async fn range_open_end() {
    let (app, _dir) = test_app(false).await;
    let resp = app
        .dispatch(get("/hello.bin").with_header("Range", "bytes=12-"))
        .await;

    assert_eq!(resp.status, StatusCode::PARTIAL_CONTENT);
    assert_eq!(resp.header("content-range").unwrap(), "bytes 12-15/16");
    assert_eq!(&body_bytes(resp).await[..], &[12u8, 13, 14, 15]);
}

#[tokio::test]
async fn range_unsatisfiable_returns_416() {
    let (app, _dir) = test_app(false).await;
    let resp = app
        .dispatch(get("/hello.bin").with_header("Range", "bytes=100-200"))
        .await;

    assert_eq!(resp.status, StatusCode::RANGE_NOT_SATISFIABLE);
    assert_eq!(resp.header("content-range").unwrap(), "bytes */16");
}

#[tokio::test]
async fn malformed_range_is_ignored() {
    let (app, _dir) = test_app(false).await;
    let resp = app
        .dispatch(get("/hello.bin").with_header("Range", "bytes=abc-def"))
        .await;

    assert_eq!(resp.status, StatusCode::OK);
    assert_eq!(body_bytes(resp).await.len(), 16);
}

#[tokio::test]
async fn if_none_match_returns_304() {
    let (app, _dir) = test_app(false).await;
    let first = app.dispatch(get("/hello.bin")).await;
    let etag = first.header("etag").unwrap().to_string();

    let resp = app
        .dispatch(get("/hello.bin").with_header("If-None-Match", etag))
        .await;
    assert_eq!(resp.status, StatusCode::NOT_MODIFIED);
}

#[tokio::test]
async fn if_none_match_star_returns_304() {
    let (app, _dir) = test_app(false).await;
    let resp = app
        .dispatch(get("/hello.bin").with_header("If-None-Match", "*"))
        .await;
    assert_eq!(resp.status, StatusCode::NOT_MODIFIED);
}

#[tokio::test]
async fn if_none_match_mismatch_returns_200() {
    let (app, _dir) = test_app(false).await;
    let resp = app
        .dispatch(
            get("/hello.bin").with_header("If-None-Match", "\"deadbeef-0\""),
        )
        .await;
    assert_eq!(resp.status, StatusCode::OK);
}

#[tokio::test]
async fn if_modified_since_returns_304() {
    let (app, _dir) = test_app(false).await;
    let now = format_http_date(std::time::SystemTime::now());
    let resp = app
        .dispatch(get("/hello.bin").with_header("If-Modified-Since", now))
        .await;
    assert_eq!(resp.status, StatusCode::NOT_MODIFIED);
}

#[tokio::test]
async fn if_modified_since_old_date_returns_200() {
    let (app, _dir) = test_app(false).await;
    let resp = app
        .dispatch(
            get("/hello.bin").with_header(
                "If-Modified-Since",
                "Sun, 06 Nov 1994 08:49:37 GMT",
            ),
        )
        .await;
    assert_eq!(resp.status, StatusCode::OK);
}

#[tokio::test]
async fn directory_listing_ok() {
    let (app, _dir) = test_app(false).await;
    let resp = app.dispatch(get("/")).await;

    assert_eq!(resp.status, StatusCode::OK);
    let html = String::from_utf8(body_bytes(resp).await).unwrap();
    assert!(html.contains("hello.bin"));
    assert!(html.contains("code.rs"));
    assert!(html.contains("subdir/"));
    assert!(!html.contains(".hidden"));
}

#[tokio::test]
async fn hidden_files_toggle() {
    let (app, _dir) = test_app(true).await;

    let resp = app.dispatch(get("/")).await;
    let html = String::from_utf8(body_bytes(resp).await).unwrap();
    assert!(html.contains(".hidden"));

    let resp = app.dispatch(get("/?hidden=false")).await;
    let html = String::from_utf8(body_bytes(resp).await).unwrap();
    assert!(!html.contains(".hidden"));
}

#[tokio::test]
async fn hidden_files_blocked_when_not_allowed() {
    let (app, _dir) = test_app(false).await;
    let resp = app.dispatch(get("/?hidden=true")).await;
    let html = String::from_utf8(body_bytes(resp).await).unwrap();
    assert!(!html.contains(".hidden"));
}

#[tokio::test]
async fn sort_by_size_orders_entries() {
    let (app, _dir) = test_app(false).await;
    let resp = app.dispatch(get("/?sort=size")).await;
    assert_eq!(resp.status, StatusCode::OK);
    let html = String::from_utf8(body_bytes(resp).await).unwrap();
    let hello_pos = html.find("hello.bin").unwrap();
    let big_pos = html.find("big.txt").unwrap();
    assert!(hello_pos < big_pos);
}

#[tokio::test]
async fn trailing_slash_directory_redirects_301_preserving_query() {
    let (app, _dir) = test_app(false).await;
    let resp = app.dispatch(get("/subdir?sort=size")).await;
    assert_eq!(resp.status, StatusCode::MOVED_PERMANENTLY);
    assert_eq!(resp.header("location"), Some("/subdir/?sort=size"));

    let plain = app.dispatch(get("/subdir")).await;
    assert_eq!(plain.status, StatusCode::MOVED_PERMANENTLY);
    assert_eq!(plain.header("location"), Some("/subdir/"));
}

#[tokio::test]
async fn nested_directory_listing() {
    let (app, _dir) = test_app(false).await;
    let resp = app.dispatch(get("/subdir/")).await;

    assert_eq!(resp.status, StatusCode::OK);
    let html = String::from_utf8(body_bytes(resp).await).unwrap();
    assert!(html.contains("nested.txt"));
}

#[tokio::test]
async fn text_file_gets_viewer_page() {
    let (app, _dir) = test_app(false).await;
    let resp = app.dispatch(get("/code.rs")).await;

    assert_eq!(resp.status, StatusCode::OK);
    let html = String::from_utf8(body_bytes(resp).await).unwrap();
    assert!(html.contains("Preview"));
    assert!(html.contains("fn main()"));
}

#[tokio::test]
async fn view_raw_serves_content() {
    let (app, _dir) = test_app(false).await;
    let resp = app.dispatch(get("/code.rs?view=raw")).await;

    assert_eq!(resp.status, StatusCode::OK);
    assert_eq!(&body_bytes(resp).await[..], b"fn main() {}\n");
}

#[tokio::test]
async fn view_download_sets_content_disposition() {
    let (app, _dir) = test_app(false).await;
    let resp = app.dispatch(get("/code.rs?view=download")).await;

    assert_eq!(resp.status, StatusCode::OK);
    let cd = resp.header("content-disposition").unwrap();
    assert!(cd.contains("attachment"));
    assert!(cd.contains("filename=\"code.rs\""));
}

#[tokio::test]
async fn download_non_ascii_filename_uses_rfc5987() {
    let (app, _dir) = test_app(false).await;
    let resp = app
        .dispatch(get("/%E4%B8%AD%E6%96%87.txt?view=download"))
        .await;

    assert_eq!(resp.status, StatusCode::OK);
    let cd = resp.header("content-disposition").unwrap();
    assert!(
        cd.contains("filename*=UTF-8''%E4%B8%AD%E6%96%87.txt"),
        "got: {cd}"
    );
}

#[tokio::test]
async fn favicon_returns_404() {
    let (app, _dir) = test_app(false).await;
    let resp = app.dispatch(get("/favicon.ico")).await;
    assert_eq!(resp.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn missing_file_returns_404() {
    let (app, _dir) = test_app(false).await;
    let resp = app.dispatch(get("/nope.bin")).await;
    assert_eq!(resp.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn path_traversal_cannot_escape_root() {
    let (app, _dir) = test_app(false).await;
    let resp = app.dispatch(get("/..%2f..%2f..%2fetc%2fpasswd")).await;
    assert_eq!(resp.status, StatusCode::NOT_FOUND);
}

#[cfg(unix)]
#[tokio::test]
async fn symlink_escape_returns_404() {
    let (app, dir) = test_app(false).await;
    std::os::unix::fs::symlink("/etc/hosts", dir.path().join("escape"))
        .unwrap();

    let resp = app.dispatch(get("/escape")).await;
    assert_eq!(resp.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn head_directory_ok() {
    let (app, _dir) = test_app(false).await;
    let resp = app
        .dispatch(HttpRequest::new(Method::Head, "/subdir/"))
        .await;
    assert_eq!(resp.status, StatusCode::OK);
}

#[tokio::test]
async fn directory_with_index_html_serves_index() {
    let (app, dir) = test_app(false).await;
    std::fs::write(dir.path().join("subdir/index.html"), "<h1>home</h1>")
        .unwrap();

    let resp = app.dispatch(get("/subdir/")).await;
    assert_eq!(resp.status, StatusCode::OK);
    let ct = resp.header("content-type").unwrap();
    assert!(ct.starts_with("text/html"), "got: {ct}");
    assert_eq!(&body_bytes(resp).await[..], b"<h1>home</h1>");
}

#[tokio::test]
async fn root_index_html_is_served() {
    let (app, dir) = test_app(false).await;
    std::fs::write(dir.path().join("index.html"), "<h1>root</h1>").unwrap();

    let resp = app.dispatch(get("/")).await;
    assert_eq!(resp.status, StatusCode::OK);
    assert_eq!(&body_bytes(resp).await[..], b"<h1>root</h1>");
}

#[tokio::test]
async fn listing_param_bypasses_index_html() {
    let (app, dir) = test_app(false).await;
    std::fs::write(dir.path().join("subdir/index.html"), "<h1>home</h1>")
        .unwrap();

    let resp = app.dispatch(get("/subdir/?listing=true")).await;
    assert_eq!(resp.status, StatusCode::OK);
    let html = String::from_utf8(body_bytes(resp).await).unwrap();
    assert!(html.contains("Index of"));
    assert!(html.contains("nested.txt"));
}

#[tokio::test]
async fn head_directory_with_index_returns_index_headers() {
    let (app, dir) = test_app(false).await;
    std::fs::write(dir.path().join("subdir/index.html"), "<h1>home</h1>")
        .unwrap();

    let resp = app
        .dispatch(HttpRequest::new(Method::Head, "/subdir/"))
        .await;
    assert_eq!(resp.status, StatusCode::OK);
    assert_eq!(resp.header("content-length").unwrap(), "13");
    assert!(body_bytes(resp).await.is_empty());
}

#[cfg(unix)]
#[tokio::test]
async fn index_html_symlink_escape_falls_back_to_listing() {
    let (app, dir) = test_app(false).await;
    std::os::unix::fs::symlink(
        "/etc/hosts",
        dir.path().join("subdir/index.html"),
    )
    .unwrap();

    let resp = app.dispatch(get("/subdir/")).await;
    assert_eq!(resp.status, StatusCode::OK);
    let html = String::from_utf8(body_bytes(resp).await).unwrap();
    assert!(html.contains("Index of"));
    assert!(html.contains("nested.txt"));
}

#[tokio::test]
async fn cors_disabled_by_default() {
    let (app, _dir) = test_app(false).await;
    let resp = app
        .dispatch(
            get("/hello.bin").with_header("Origin", "https://example.com"),
        )
        .await;

    assert_eq!(resp.status, StatusCode::OK);
    assert!(resp.header("access-control-allow-origin").is_none());
}

#[tokio::test]
async fn cors_enabled_allows_any_origin() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("a.bin"), b"ok").unwrap();
    let app = create_app_with_options(
        dir.path().canonicalize().unwrap(),
        Options {
            allow_hidden: false,
            cors: true,
        },
    )
    .await;

    let resp = app
        .dispatch(get("/a.bin").with_header("Origin", "https://example.com"))
        .await;
    assert_eq!(resp.status, StatusCode::OK);
    assert_eq!(resp.header("access-control-allow-origin").unwrap(), "*");

    let opt = app
        .dispatch(
            HttpRequest::new(Method::Options, "/a.bin")
                .with_header("Origin", "https://example.com")
                .with_header("Access-Control-Request-Method", "GET"),
        )
        .await;
    assert_eq!(opt.status, StatusCode::NO_CONTENT);
    assert_eq!(opt.header("access-control-allow-origin").unwrap(), "*");
}

#[tokio::test]
async fn compression_negotiation_and_206_exemption() {
    let (app, _dir) = test_app(false).await;

    let compressed = app
        .dispatch(
            get("/big.txt?view=raw").with_header("Accept-Encoding", "br, gzip"),
        )
        .await;
    assert_eq!(compressed.status, StatusCode::OK);
    assert_eq!(compressed.header("content-encoding"), Some("br"));

    let range_resp = app
        .dispatch(
            get("/big.bin")
                .with_header("Range", "bytes=0-31")
                .with_header("Accept-Encoding", "br, gzip"),
        )
        .await;
    assert_eq!(range_resp.status, StatusCode::PARTIAL_CONTENT);
    assert!(
        range_resp.header("content-encoding").is_none(),
        "206/stream range must remain exempt from compression"
    );
}

#[test]
fn workspace_dependency_purity_and_legacy_removed() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    assert!(
        !root.join("src/httpd").exists(),
        "legacy src/httpd must be removed"
    );
    assert!(!root.join("src/error.rs").exists());
    assert!(!root.join("src/utils.rs").exists());

    let root_toml = std::fs::read_to_string(root.join("Cargo.toml")).unwrap();
    for banned in [
        "axum",
        "tower",
        "clap",
        "serde",
        "httpdate",
        "mime_guess",
        "percent-encoding",
        "tracing",
    ] {
        assert!(
            !root_toml.contains(banned),
            "root Cargo.toml must not depend on {banned}"
        );
    }

    for zero_dep in [
        "crates/frame/Cargo.toml",
        "crates/contracts/net-api/Cargo.toml",
        "crates/contracts/http-api/Cargo.toml",
        "crates/contracts/files-api/Cargo.toml",
        "crates/contracts/render-api/Cargo.toml",
        "crates/components/ext-cors/Cargo.toml",
        "crates/entries/cli/Cargo.toml",
    ] {
        let toml = std::fs::read_to_string(root.join(zero_dep)).unwrap();
        let deps_section = toml.split("[dependencies]").nth(1).unwrap_or("");
        for line in deps_section.lines().map(str::trim) {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if line.starts_with('[') {
                break;
            }
            assert!(
                line.contains("path ="),
                "{zero_dep} must have zero external dependencies, found: {line}"
            );
        }
    }
}
