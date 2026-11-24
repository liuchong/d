//! System integration tests for `d` exercised against the real assembled
//! router pipeline (`AppHandle::dispatch`), plus workspace hygiene gates.

use app_files::format_http_date;
use d::{AppHandle, Options, create_app, create_app_with_options};
use frame::{ConfigValue, EntryPatch, EntryTarget, ErrorKind, PluginEntry};
use http_api::{HttpRequest, HttpResponse, Method, StatusCode};
use std::collections::BTreeMap;
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
    assert!(html.contains("<span class=\"hl-kw\">fn</span>"));
    assert!(html.contains("<span class=\"hl-fn\">main</span>"));
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

#[tokio::test]
async fn markdown_html_passthrough_and_relative_images() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    std::fs::create_dir(root.join("docs")).unwrap();
    std::fs::write(
        root.join("docs/diagram.svg"),
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"64\" height=\"64\"></svg>",
    )
    .unwrap();
    std::fs::write(
        root.join("docs/guide.md"),
        "# 架构 Overview\n\n![架构图](./diagram.svg)\n\n<details><summary>展开</summary>\n\n说明文本 <kbd>Ctrl</kbd>\n\n</details>\n",
    )
    .unwrap();

    let app = create_app(root.canonicalize().unwrap(), false).await;

    // Without trailing slash, /docs redirects to /docs/ so relative `./diagram.svg` resolves properly.
    let redir = app.dispatch(get("/docs")).await;
    assert_eq!(redir.status, StatusCode::MOVED_PERMANENTLY);
    assert_eq!(redir.header("location"), Some("/docs/"));

    let md_resp = app.dispatch(get("/docs/guide.md")).await;
    assert_eq!(md_resp.status, StatusCode::OK);
    let html = String::from_utf8(body_bytes(md_resp).await).unwrap();
    assert!(html.contains("<h1 id=\"架构-overview\">"), "{html}");
    assert!(
        html.contains("<img src=\"./diagram.svg\" alt=\"架构图\""),
        "{html}"
    );
    assert!(html.contains("<details><summary>展开</summary>"), "{html}");
    assert!(html.contains("<kbd>Ctrl</kbd>"), "{html}");
    assert!(!html.contains("&lt;details&gt;"), "{html}");

    let img_resp = app.dispatch(get("/docs/diagram.svg")).await;
    assert_eq!(img_resp.status, StatusCode::OK);
    assert_eq!(img_resp.header("content-type"), Some("image/svg+xml"));
}

#[tokio::test]
async fn viewer_modes_and_no_external_cdn() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    std::fs::write(root.join("note.md"), "# Hello\n\nBody\n").unwrap();
    std::fs::write(root.join("code.rs"), "fn main() {}\n").unwrap();
    let app = create_app(root.canonicalize().unwrap(), false).await;

    for uri in ["/", "/note.md", "/code.rs"] {
        let resp = app.dispatch(get(uri)).await;
        assert_eq!(resp.status, StatusCode::OK);
        let html = String::from_utf8(body_bytes(resp).await).unwrap();
        assert!(!html.contains("<script"), "no external scripts on {uri}");
        assert!(
            !html.contains("<link") || !html.contains("http"),
            "no remote stylesheet links on {uri}"
        );
    }

    let viewer = String::from_utf8(
        body_bytes(app.dispatch(get("/note.md")).await).await,
    )
    .unwrap();
    assert!(viewer.contains("href=\"/note.md?view=raw\""), "{viewer}");
    assert!(
        viewer.contains("href=\"/note.md?view=download\""),
        "{viewer}"
    );

    let preview = String::from_utf8(
        body_bytes(app.dispatch(get("/note.md?view=preview")).await).await,
    )
    .unwrap();
    assert!(preview.contains("<h1 id=\"hello\">"), "{preview}");

    let raw = String::from_utf8(
        body_bytes(app.dispatch(get("/note.md?view=raw")).await).await,
    )
    .unwrap();
    assert_eq!(raw, "# Hello\n\nBody\n");
}

#[tokio::test]
async fn code_viewer_server_side_highlighting() {
    let (app, _dir) = test_app(false).await;
    let resp = app.dispatch(get("/code.rs")).await;
    assert_eq!(resp.status, StatusCode::OK);
    let html = String::from_utf8(body_bytes(resp).await).unwrap();
    assert!(html.contains("<span class=\"hl-kw\">fn</span>"), "{html}");
    assert!(html.contains(".hl-kw"), "{html}");
}

#[tokio::test]
async fn media_raw_and_viewer() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    std::fs::create_dir(root.join("media")).unwrap();
    std::fs::write(root.join("media/logo.svg"), "<svg></svg>").unwrap();
    std::fs::write(root.join("media/clip.mp4"), b"fake-mp4").unwrap();
    std::fs::write(root.join("media/song.mp3"), b"fake-mp3").unwrap();
    let app = create_app(root.canonicalize().unwrap(), false).await;

    // Bare media paths ALWAYS stream raw bytes.
    let raw_svg = app.dispatch(get("/media/logo.svg")).await;
    assert_eq!(raw_svg.status, StatusCode::OK);
    assert_eq!(raw_svg.header("content-type"), Some("image/svg+xml"));
    assert_eq!(&body_bytes(raw_svg).await[..], b"<svg></svg>");

    // Directory listing points media items to ?view=viewer.
    let dir_html =
        String::from_utf8(body_bytes(app.dispatch(get("/media/")).await).await)
            .unwrap();
    assert!(
        dir_html.contains("/media/logo.svg?view=viewer"),
        "{dir_html}"
    );

    // ?view=viewer opens white minimalist HTML viewer embedding the bare stream path.
    let img_viewer = String::from_utf8(
        body_bytes(app.dispatch(get("/media/logo.svg?view=viewer")).await)
            .await,
    )
    .unwrap();
    assert!(
        img_viewer.contains("<img src=\"/media/logo.svg\""),
        "{img_viewer}"
    );

    let vid_viewer = String::from_utf8(
        body_bytes(app.dispatch(get("/media/clip.mp4?view=viewer")).await)
            .await,
    )
    .unwrap();
    assert!(vid_viewer.contains("<video controls"), "{vid_viewer}");

    let aud_viewer = String::from_utf8(
        body_bytes(app.dispatch(get("/media/song.mp3?view=viewer")).await)
            .await,
    )
    .unwrap();
    assert!(aud_viewer.contains("<audio controls"), "{aud_viewer}");
}

#[tokio::test]
async fn white_minimal_ui() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    std::fs::write(root.join("doc.md"), "# Title\n").unwrap();
    std::fs::write(root.join("src.rs"), "fn f() {}\n").unwrap();
    std::fs::write(root.join("pic.svg"), "<svg></svg>").unwrap();
    let app = create_app(root.canonicalize().unwrap(), false).await;

    for uri in ["/", "/doc.md", "/src.rs", "/pic.svg?view=viewer"] {
        let html =
            String::from_utf8(body_bytes(app.dispatch(get(uri)).await).await)
                .unwrap();
        assert!(html.contains("#ffffff"), "missing #ffffff on {uri}");
        assert!(html.contains("#f6f8fa"), "missing #f6f8fa on {uri}");
        assert!(html.contains("#0969da"), "missing #0969da on {uri}");
        for forbidden in [
            "#0d1117",
            "#161b22",
            "file-type-badge",
            "📁",
            "📜",
            "🎬",
            "⬇️",
        ] {
            assert!(
                !html.contains(forbidden),
                "forbidden dark/emoji token {forbidden} present on {uri}"
            );
        }
    }
}

#[tokio::test]
async fn loader_hot_patch_cors_and_config_without_downtime() {
    let dir = TempDir::new().unwrap();
    let root = dir.path().canonicalize().unwrap();
    std::fs::write(root.join("hello.bin"), b"ok-payload").unwrap();
    std::fs::write(root.join(".hidden"), b"secret").unwrap();

    let app = create_app(root.clone(), false).await;

    let proto_fiber_before = app
        .inspect()
        .fibers
        .iter()
        .find(|f| f.name == "proto-http")
        .map(|f| f.id)
        .expect("proto-http fiber present");

    // Initially CORS is disabled and hidden files are excluded.
    let no_cors = app
        .dispatch(
            get("/hello.bin").with_header("Origin", "https://example.com"),
        )
        .await;
    assert_eq!(no_cors.status, StatusCode::OK);
    assert!(no_cors.header("access-control-allow-origin").is_none());

    let list_before =
        String::from_utf8(body_bytes(app.dispatch(get("/")).await).await)
            .unwrap();
    assert!(!list_before.contains(".hidden"));

    // Hot-insert ext-cors via Loader::apply_patches without touching proto-http.
    app.loader()
        .apply_patches(vec![EntryPatch::Insert {
            anchor: None,
            entry: PluginEntry {
                id: "ext-cors".to_string(),
                target: EntryTarget::Name("ext-cors".to_string()),
                config: ConfigValue::Null,
                disabled: false,
                isolate: Vec::new(),
                children: Vec::new(),
            },
        }])
        .await
        .unwrap();

    let cors_get = app
        .dispatch(
            get("/hello.bin").with_header("Origin", "https://example.com"),
        )
        .await;
    assert_eq!(cors_get.status, StatusCode::OK);
    assert_eq!(cors_get.header("access-control-allow-origin"), Some("*"));

    let cors_opt = app
        .dispatch(
            HttpRequest::new(Method::Options, "/hello.bin")
                .with_header("Origin", "https://example.com")
                .with_header("Access-Control-Request-Method", "GET"),
        )
        .await;
    assert_eq!(cors_opt.status, StatusCode::NO_CONTENT);
    assert_eq!(cors_opt.header("access-control-allow-origin"), Some("*"));

    // Hot-remove ext-cors and enable allow_hidden in app-files simultaneously.
    app.loader()
        .apply_patches(vec![
            EntryPatch::Remove {
                id: "ext-cors".to_string(),
            },
            EntryPatch::ReplaceConfig {
                id: "app-files".to_string(),
                config: ConfigValue::Map(BTreeMap::from([
                    (
                        "root".to_string(),
                        ConfigValue::Str(root.to_string_lossy().into_owned()),
                    ),
                    ("allow_hidden".to_string(), ConfigValue::Bool(true)),
                ])),
                disabled: None,
            },
        ])
        .await
        .unwrap();
    app.context().settle().await;

    let cors_removed = app
        .dispatch(
            get("/hello.bin").with_header("Origin", "https://example.com"),
        )
        .await;
    assert_eq!(cors_removed.status, StatusCode::OK);
    assert!(cors_removed.header("access-control-allow-origin").is_none());

    let list_after =
        String::from_utf8(body_bytes(app.dispatch(get("/")).await).await)
            .unwrap();
    assert!(list_after.contains(".hidden"));

    // Invalid config patch is rejected before commit without disrupting HTTP service.
    let bad_patch = app
        .loader()
        .apply_patches(vec![EntryPatch::ReplaceConfig {
            id: "app-files".to_string(),
            config: ConfigValue::Map(BTreeMap::from([
                (
                    "root".to_string(),
                    ConfigValue::Str(root.to_string_lossy().into_owned()),
                ),
                (
                    "allow_hidden".to_string(),
                    ConfigValue::Str("not-a-bool".to_string()),
                ),
            ])),
            disabled: None,
        }])
        .await
        .unwrap_err();
    assert_eq!(bad_patch.kind, ErrorKind::ValidationFailed);

    let still_serving = app.dispatch(get("/hello.bin")).await;
    assert_eq!(still_serving.status, StatusCode::OK);

    let proto_fiber_after = app
        .inspect()
        .fibers
        .iter()
        .find(|f| f.name == "proto-http")
        .map(|f| f.id)
        .expect("proto-http fiber still active");
    assert_eq!(
        proto_fiber_before, proto_fiber_after,
        "proto-http must stay running without restart across ext/config patches"
    );

    app.dispose().await.unwrap();
}
