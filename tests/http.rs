//! Integration tests for the HTTP behaviour of `d`, exercised in-process
//! via `tower::ServiceExt::oneshot` (no real sockets needed).

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use d::httpd::{Options, create_app, create_app_with_options};
use tower::ServiceExt;

/// Build a server root with a small file tree and return the app plus the
/// tempdir guard (dropping the guard deletes the tree).
fn test_app(allow_hidden: bool) -> (axum::Router, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    std::fs::write(root.join("hello.bin"), (0u8..16).collect::<Vec<u8>>())
        .unwrap();
    std::fs::write(root.join("code.rs"), "fn main() {}\n").unwrap();
    std::fs::write(root.join("中文.txt"), "你好，世界").unwrap();
    std::fs::write(root.join(".hidden"), "secret").unwrap();
    std::fs::create_dir(root.join("subdir")).unwrap();
    std::fs::write(root.join("subdir/nested.txt"), "nested").unwrap();

    // The server expects an already-canonical root (as produced by main).
    let canonical = root.canonicalize().unwrap();
    (create_app(canonical, allow_hidden), dir)
}

fn get(uri: &str) -> Request<Body> {
    Request::builder().uri(uri).body(Body::empty()).unwrap()
}

async fn body_bytes(response: axum::response::Response) -> Vec<u8> {
    axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap()
        .to_vec()
}

#[tokio::test]
async fn get_file_ok() {
    let (app, _dir) = test_app(false);
    let resp = app.oneshot(get("/hello.bin")).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.headers().get(header::CONTENT_LENGTH).unwrap(), "16");
    assert_eq!(resp.headers().get(header::ACCEPT_RANGES).unwrap(), "bytes");
    assert!(resp.headers().contains_key(header::ETAG));
    assert!(resp.headers().contains_key(header::LAST_MODIFIED));

    let body = body_bytes(resp).await;
    assert_eq!(&body[..], &(0u8..16).collect::<Vec<u8>>()[..]);
}

#[tokio::test]
async fn head_file_headers_only() {
    let (app, _dir) = test_app(false);
    let req = Request::builder()
        .method("HEAD")
        .uri("/hello.bin")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.headers().get(header::CONTENT_LENGTH).unwrap(), "16");
    assert!(body_bytes(resp).await.is_empty());
}

#[tokio::test]
async fn range_request_returns_slice() {
    let (app, _dir) = test_app(false);
    let req = Request::builder()
        .uri("/hello.bin")
        .header(header::RANGE, "bytes=5-9")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();

    assert_eq!(resp.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(
        resp.headers().get(header::CONTENT_RANGE).unwrap(),
        "bytes 5-9/16"
    );
    assert_eq!(resp.headers().get(header::CONTENT_LENGTH).unwrap(), "5");

    let body = body_bytes(resp).await;
    assert_eq!(&body[..], &[5u8, 6, 7, 8, 9]);
}

#[tokio::test]
async fn range_suffix_returns_tail() {
    let (app, _dir) = test_app(false);
    let req = Request::builder()
        .uri("/hello.bin")
        .header(header::RANGE, "bytes=-4")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();

    assert_eq!(resp.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(
        resp.headers().get(header::CONTENT_RANGE).unwrap(),
        "bytes 12-15/16"
    );
    let body = body_bytes(resp).await;
    assert_eq!(&body[..], &[12u8, 13, 14, 15]);
}

#[tokio::test]
async fn range_open_end() {
    let (app, _dir) = test_app(false);
    let req = Request::builder()
        .uri("/hello.bin")
        .header(header::RANGE, "bytes=14-")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();

    assert_eq!(resp.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(
        resp.headers().get(header::CONTENT_RANGE).unwrap(),
        "bytes 14-15/16"
    );
    let body = body_bytes(resp).await;
    assert_eq!(&body[..], &[14u8, 15]);
}

#[tokio::test]
async fn range_unsatisfiable_returns_416() {
    let (app, _dir) = test_app(false);
    let req = Request::builder()
        .uri("/hello.bin")
        .header(header::RANGE, "bytes=999-")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();

    assert_eq!(resp.status(), StatusCode::RANGE_NOT_SATISFIABLE);
    assert_eq!(
        resp.headers().get(header::CONTENT_RANGE).unwrap(),
        "bytes */16"
    );
}

#[tokio::test]
async fn malformed_range_is_ignored() {
    let (app, _dir) = test_app(false);
    let req = Request::builder()
        .uri("/hello.bin")
        .header(header::RANGE, "bytes=abc-def")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(body_bytes(resp).await.len(), 16);
}

#[tokio::test]
async fn if_none_match_returns_304() {
    let (app, _dir) = test_app(false);

    let resp = app.clone().oneshot(get("/hello.bin")).await.unwrap();
    let etag = resp
        .headers()
        .get(header::ETAG)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();

    let req = Request::builder()
        .uri("/hello.bin")
        .header(header::IF_NONE_MATCH, &etag)
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();

    assert_eq!(resp.status(), StatusCode::NOT_MODIFIED);
    assert!(resp.headers().contains_key(header::ETAG));
    assert!(body_bytes(resp).await.is_empty());
}

#[tokio::test]
async fn if_none_match_star_returns_304() {
    let (app, _dir) = test_app(false);
    let req = Request::builder()
        .uri("/hello.bin")
        .header(header::IF_NONE_MATCH, "*")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();

    assert_eq!(resp.status(), StatusCode::NOT_MODIFIED);
}

#[tokio::test]
async fn if_none_match_mismatch_returns_200() {
    let (app, _dir) = test_app(false);
    let req = Request::builder()
        .uri("/hello.bin")
        .header(header::IF_NONE_MATCH, "\"deadbeef-0\"")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn if_modified_since_returns_304() {
    let (app, _dir) = test_app(false);
    let now = httpdate::fmt_http_date(std::time::SystemTime::now());
    let req = Request::builder()
        .uri("/hello.bin")
        .header(header::IF_MODIFIED_SINCE, now)
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();

    assert_eq!(resp.status(), StatusCode::NOT_MODIFIED);
}

#[tokio::test]
async fn if_modified_since_old_date_returns_200() {
    let (app, _dir) = test_app(false);
    let req = Request::builder()
        .uri("/hello.bin")
        .header(header::IF_MODIFIED_SINCE, "Sun, 06 Nov 1994 08:49:37 GMT")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn directory_listing_ok() {
    let (app, _dir) = test_app(false);
    let resp = app.oneshot(get("/")).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_bytes(resp).await;
    let html = String::from_utf8(body).unwrap();
    assert!(html.contains("hello.bin"));
    assert!(html.contains("code.rs"));
    assert!(html.contains("subdir/"));
    // Hidden files are not shown by default.
    assert!(!html.contains(".hidden"));
}

#[tokio::test]
async fn hidden_files_toggle() {
    let (app, _dir) = test_app(true);

    let resp = app.clone().oneshot(get("/")).await.unwrap();
    let html = String::from_utf8(body_bytes(resp).await).unwrap();
    // allow_hidden defaults the listing to showing hidden files.
    assert!(html.contains(".hidden"));

    let resp = app.oneshot(get("/?hidden=false")).await.unwrap();
    let html = String::from_utf8(body_bytes(resp).await).unwrap();
    assert!(!html.contains(".hidden"));
}

#[tokio::test]
async fn hidden_files_blocked_when_not_allowed() {
    let (app, _dir) = test_app(false);
    let resp = app.oneshot(get("/?hidden=true")).await.unwrap();
    let html = String::from_utf8(body_bytes(resp).await).unwrap();
    assert!(!html.contains(".hidden"));
}

#[tokio::test]
async fn nested_directory_listing() {
    let (app, _dir) = test_app(false);
    let resp = app.oneshot(get("/subdir/")).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let html = String::from_utf8(body_bytes(resp).await).unwrap();
    assert!(html.contains("nested.txt"));
}

#[tokio::test]
async fn text_file_gets_viewer_page() {
    let (app, _dir) = test_app(false);
    let resp = app.oneshot(get("/code.rs")).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let html = String::from_utf8(body_bytes(resp).await).unwrap();
    assert!(html.contains("Preview"));
    assert!(html.contains("fn main()"));
}

#[tokio::test]
async fn view_raw_serves_content() {
    let (app, _dir) = test_app(false);
    let resp = app.oneshot(get("/code.rs?view=raw")).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(&body_bytes(resp).await[..], b"fn main() {}\n");
}

#[tokio::test]
async fn view_download_sets_content_disposition() {
    let (app, _dir) = test_app(false);
    let resp = app.oneshot(get("/code.rs?view=download")).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let cd = resp
        .headers()
        .get(header::CONTENT_DISPOSITION)
        .unwrap()
        .to_str()
        .unwrap();
    assert!(cd.contains("attachment"));
    assert!(cd.contains("filename=\"code.rs\""));
}

#[tokio::test]
async fn download_non_ascii_filename_uses_rfc5987() {
    let (app, _dir) = test_app(false);
    let resp = app
        .oneshot(get("/%E4%B8%AD%E6%96%87.txt?view=download"))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let cd = resp
        .headers()
        .get(header::CONTENT_DISPOSITION)
        .unwrap()
        .to_str()
        .unwrap();
    assert!(
        cd.contains("filename*=UTF-8''%E4%B8%AD%E6%96%87.txt"),
        "got: {}",
        cd
    );
}

#[tokio::test]
async fn favicon_returns_404() {
    let (app, _dir) = test_app(false);
    let resp = app.oneshot(get("/favicon.ico")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn missing_file_returns_404() {
    let (app, _dir) = test_app(false);
    let resp = app.oneshot(get("/nope.bin")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn path_traversal_cannot_escape_root() {
    let (app, _dir) = test_app(false);
    // `..` components are clamped at the root, so this resolves to
    // `<root>/etc/passwd` which does not exist: 404, never /etc/passwd.
    let resp = app
        .oneshot(get("/..%2f..%2f..%2fetc%2fpasswd"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[cfg(unix)]
#[tokio::test]
async fn symlink_escape_returns_404() {
    let (app, dir) = test_app(false);
    std::os::unix::fs::symlink("/etc/hosts", dir.path().join("escape"))
        .unwrap();

    let resp = app.oneshot(get("/escape")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn head_directory_ok() {
    let (app, _dir) = test_app(false);
    let req = Request::builder()
        .method("HEAD")
        .uri("/subdir/")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn directory_with_index_html_serves_index() {
    let (app, dir) = test_app(false);
    std::fs::write(dir.path().join("subdir/index.html"), "<h1>home</h1>")
        .unwrap();

    let resp = app.oneshot(get("/subdir/")).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let content_type = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .unwrap()
        .to_str()
        .unwrap();
    assert!(content_type.starts_with("text/html"), "got: {content_type}");
    assert_eq!(&body_bytes(resp).await[..], b"<h1>home</h1>");
}

#[tokio::test]
async fn root_index_html_is_served() {
    let (app, dir) = test_app(false);
    std::fs::write(dir.path().join("index.html"), "<h1>root</h1>").unwrap();

    let resp = app.oneshot(get("/")).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(&body_bytes(resp).await[..], b"<h1>root</h1>");
}

#[tokio::test]
async fn listing_param_bypasses_index_html() {
    let (app, dir) = test_app(false);
    std::fs::write(dir.path().join("subdir/index.html"), "<h1>home</h1>")
        .unwrap();

    let resp = app.oneshot(get("/subdir/?listing=true")).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let html = String::from_utf8(body_bytes(resp).await).unwrap();
    assert!(html.contains("Index of"));
    assert!(html.contains("nested.txt"));
}

#[tokio::test]
async fn head_directory_with_index_returns_index_headers() {
    let (app, dir) = test_app(false);
    std::fs::write(dir.path().join("subdir/index.html"), "<h1>home</h1>")
        .unwrap();

    let req = Request::builder()
        .method("HEAD")
        .uri("/subdir/")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.headers().get(header::CONTENT_LENGTH).unwrap(), "13");
    assert!(body_bytes(resp).await.is_empty());
}

#[cfg(unix)]
#[tokio::test]
async fn index_html_symlink_escape_falls_back_to_listing() {
    let (app, dir) = test_app(false);
    std::os::unix::fs::symlink(
        "/etc/hosts",
        dir.path().join("subdir/index.html"),
    )
    .unwrap();

    let resp = app.oneshot(get("/subdir/")).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let html = String::from_utf8(body_bytes(resp).await).unwrap();
    // The listing is shown instead of the escaped symlink target.
    assert!(html.contains("Index of"));
    assert!(html.contains("nested.txt"));
}

#[tokio::test]
async fn cors_disabled_by_default() {
    let (app, _dir) = test_app(false);
    let req = Request::builder()
        .uri("/hello.bin")
        .header(header::ORIGIN, "https://example.com")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    assert!(
        resp.headers()
            .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .is_none()
    );
}

#[tokio::test]
async fn cors_enabled_allows_any_origin() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("hello.bin"), b"hi").unwrap();
    let canonical = dir.path().canonicalize().unwrap();
    let app = create_app_with_options(
        canonical,
        Options {
            allow_hidden: false,
            cors: true,
        },
    );

    let req = Request::builder()
        .uri("/hello.bin")
        .header(header::ORIGIN, "https://example.com")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers()
            .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .unwrap(),
        "*"
    );
}
