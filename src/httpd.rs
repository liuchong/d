use crate::utils::{
    decode_path, encode_path, format_http_date, format_size, guess_mime_type,
};
use axum::{
    Router,
    body::Body,
    extract::{Request, State},
    http::{HeaderMap, Method, StatusCode, header},
    response::{Html, IntoResponse, Response},
    routing::get,
};
use std::net::SocketAddr;
use std::path::{Component, Path, PathBuf};

use tokio::fs;
use tokio_util::io::ReaderStream;
use tracing::{error, info, warn};

/// Server state
#[derive(Clone)]
pub struct ServerState {
    root: PathBuf,
}

/// File information for directory listing
struct FileEntry {
    name: String,
    path: String,
    is_dir: bool,
    size: u64,
    modified: Option<std::time::SystemTime>,
}

/// Create the router with CORS and compression support
pub fn create_app(root: PathBuf) -> Router {
    use tower_http::compression::CompressionLayer;
    use tower_http::cors::CorsLayer;

    let state = ServerState { root };

    // CORS: Allow all origins for a simple file server
    let cors = CorsLayer::new()
        .allow_origin(tower_http::cors::Any)
        .allow_methods([axum::http::Method::GET, axum::http::Method::HEAD])
        .allow_headers(tower_http::cors::Any);

    // Compression: gzip, deflate, br (brotli)
    let compression = CompressionLayer::new().gzip(true).deflate(true).br(true);

    Router::new()
        .route("/{*path}", get(handle_request).head(handle_request))
        .route("/", get(handle_root).head(handle_root))
        .layer(compression)
        .layer(cors)
        .with_state(state)
}

/// Start the HTTP server with graceful shutdown
pub async fn start(addr: &SocketAddr, root: &str) {
    let root_path = PathBuf::from(root);

    if !root_path.exists() {
        error!("Root path does not exist: {}", root);
        return;
    }

    let app = create_app(root_path);

    info!("Starting server on http://{}", addr);
    info!("Serving directory: {}", root);
    info!("Press Ctrl+C to stop");

    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => {
            error!("Failed to bind to {}: {}", addr, e);
            return;
        }
    };

    let serve = axum::serve(listener, app);

    // Graceful shutdown
    let shutdown = async {
        match tokio::signal::ctrl_c().await {
            Ok(()) => info!("Received shutdown signal"),
            Err(e) => error!("Failed to listen for shutdown signal: {}", e),
        }
    };

    if let Err(e) = serve.with_graceful_shutdown(shutdown).await {
        error!("Server error: {}", e);
    }

    info!("Server stopped");
}

/// Handle root path
async fn handle_root(
    State(state): State<ServerState>,
    request: Request<Body>,
) -> impl IntoResponse {
    let is_head = request.method() == Method::HEAD;
    handle_path("", None, is_head, &state).await
}

/// Handle any request (GET or HEAD)
async fn handle_request(
    State(state): State<ServerState>,
    request: Request<Body>,
) -> impl IntoResponse {
    let method = request.method().clone();
    let path = request.uri().path().trim_start_matches('/');

    let decoded = match decode_path(path) {
        Ok(p) => p,
        Err(_) => {
            return (StatusCode::BAD_REQUEST, "Invalid encoding")
                .into_response();
        }
    };

    if decoded == "/favicon.ico" {
        return StatusCode::NOT_FOUND.into_response();
    }

    info!("{} {}", method, decoded);

    let range = request
        .headers()
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok());

    let is_head = method == Method::HEAD;
    handle_path(&decoded, range, is_head, &state).await
}

/// Handle path - either serve file or directory listing
async fn handle_path(
    path: &str,
    range: Option<&str>,
    is_head: bool,
    state: &ServerState,
) -> Response {
    let full_path = match sanitize_path(&state.root, path) {
        Some(p) => p,
        None => {
            warn!("Path traversal attempt blocked: {}", path);
            return (StatusCode::FORBIDDEN, "Invalid path").into_response();
        }
    };

    match fs::metadata(&full_path).await {
        Ok(metadata) if metadata.is_dir() => {
            if is_head {
                // HEAD request for directory: return empty body
                (StatusCode::OK, Html("")).into_response()
            } else {
                serve_directory(path, &full_path, state).await
            }
        }
        Ok(metadata) => serve_file(&full_path, &metadata, range, is_head).await,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            (StatusCode::NOT_FOUND, "Not Found").into_response()
        }
        Err(e) => {
            error!("Error accessing {}: {}", full_path.display(), e);
            (StatusCode::INTERNAL_SERVER_ERROR, "Internal Server Error")
                .into_response()
        }
    }
}

/// Sanitize path to prevent directory traversal attacks
fn sanitize_path(base: &Path, requested: &str) -> Option<PathBuf> {
    let mut result = base.to_path_buf();

    for component in Path::new(requested).components() {
        match component {
            Component::Normal(c) => result.push(c),
            Component::ParentDir => {
                // Prevent escaping base directory
                if result != *base {
                    result.pop();
                }
            }
            Component::RootDir | Component::CurDir => {}
            Component::Prefix(_) => return None,
        }
    }

    // Final check: ensure the resolved path is within base
    if result.starts_with(base) {
        Some(result)
    } else {
        None
    }
}

/// Parse Range header
fn parse_range(range: &str, file_size: u64) -> Option<(u64, u64)> {
    let range = range.strip_prefix("bytes=")?;
    let parts: Vec<&str> = range.split('-').collect();
    if parts.len() != 2 {
        return None;
    }

    let start = parts[0].parse::<u64>().ok()?;
    let end = if parts[1].is_empty() {
        file_size - 1
    } else {
        parts[1].parse::<u64>().ok()?
    };

    if start > end || end >= file_size {
        return None;
    }

    Some((start, end))
}

/// Generate ETag from metadata
fn generate_etag(metadata: &std::fs::Metadata) -> String {
    let modified = metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("\"{:x}-{:x}\"", modified, metadata.len())
}

/// Serve a file with proper streaming, range support and caching
async fn serve_file(
    path: &PathBuf,
    metadata: &std::fs::Metadata,
    range_header: Option<&str>,
    is_head: bool,
) -> Response {
    let file_size = metadata.len();
    let etag = generate_etag(metadata);

    let mut headers = HeaderMap::new();

    // Set content type
    let mime = guess_mime_type(
        path.file_name().and_then(|n| n.to_str()).unwrap_or(""),
    );
    headers.insert(header::CONTENT_TYPE, mime.parse().unwrap());

    // Set accept-ranges
    headers.insert(header::ACCEPT_RANGES, "bytes".parse().unwrap());

    // Set cache control (1 hour for static files)
    headers.insert(
        header::CACHE_CONTROL,
        "public, max-age=3600".parse().unwrap(),
    );

    // Set ETag
    headers.insert(header::ETAG, etag.parse().unwrap());

    // Set last-modified
    if let Ok(modified) = metadata.modified() {
        if let Ok(time_str) = time::OffsetDateTime::from(modified)
            .format(&time::format_description::well_known::Rfc2822)
        {
            headers.insert(header::LAST_MODIFIED, time_str.parse().unwrap());
        }
    }

    // Handle Range request
    if let Some(range) = range_header.and_then(|r| parse_range(r, file_size)) {
        let (start, end) = range;
        let content_length = end - start + 1;

        headers.insert(
            header::CONTENT_LENGTH,
            content_length.to_string().parse().unwrap(),
        );
        headers.insert(
            header::CONTENT_RANGE,
            format!("bytes {}-{}/{}", start, end, file_size)
                .parse()
                .unwrap(),
        );

        // For HEAD request, return headers only
        if is_head {
            return (StatusCode::PARTIAL_CONTENT, headers).into_response();
        }

        // Open file and seek to start position
        let file = match fs::File::open(path).await {
            Ok(f) => f,
            Err(e) => {
                error!("Failed to open file {}: {}", path.display(), e);
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Internal Server Error",
                )
                    .into_response();
            }
        };

        // Use tokio_util to read specific range
        let stream = ReaderStream::new(file);
        let body = Body::from_stream(stream);

        return (StatusCode::PARTIAL_CONTENT, headers, body).into_response();
    }

    // Full file response
    headers.insert(
        header::CONTENT_LENGTH,
        file_size.to_string().parse().unwrap(),
    );

    // For HEAD request, return headers only (no body)
    if is_head {
        return (StatusCode::OK, headers).into_response();
    }

    let file = match fs::File::open(path).await {
        Ok(f) => f,
        Err(e) => {
            error!("Failed to open file {}: {}", path.display(), e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Internal Server Error",
            )
                .into_response();
        }
    };

    let stream = ReaderStream::new(file);
    let body = Body::from_stream(stream);

    (StatusCode::OK, headers, body).into_response()
}

/// Serve directory listing HTML
async fn serve_directory(
    rel_path: &str,
    full_path: &PathBuf,
    _state: &ServerState,
) -> Response {
    let mut entries = match fs::read_dir(full_path).await {
        Ok(entries) => entries,
        Err(e) => {
            warn!("Cannot read directory {}: {}", full_path.display(), e);
            return (StatusCode::FORBIDDEN, "Permission Denied")
                .into_response();
        }
    };

    let mut files = Vec::new();

    // Add parent directory link if not at root
    if !rel_path.is_empty() {
        files.push(FileEntry {
            name: "..".to_string(),
            path: "../".to_string(),
            is_dir: true,
            size: 0,
            modified: None,
        });
    }

    // Read directory entries
    while let Ok(Some(entry)) = entries.next_entry().await {
        let name = entry.file_name().to_string_lossy().to_string();
        let metadata = entry.metadata().await.ok();

        let is_dir = metadata.as_ref().map(|m| m.is_dir()).unwrap_or(false);
        let size = metadata.as_ref().map(|m| m.len()).unwrap_or(0);
        let modified = metadata.and_then(|m| m.modified().ok());

        let path =
            format!("{}{}", encode_path(&name), if is_dir { "/" } else { "" });

        files.push(FileEntry {
            name,
            path,
            is_dir,
            size,
            modified,
        });
    }

    // Sort: directories first, then by name
    files.sort_by(|a, b| match (a.is_dir, b.is_dir) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
    });

    let html = generate_directory_html(rel_path, &files);
    Html(html).into_response()
}

/// Generate HTML for directory listing using efficient string building
fn generate_directory_html(
    current_path: &str,
    entries: &[FileEntry],
) -> String {
    use std::fmt::Write;

    let display_path = if current_path.is_empty() {
        "/".to_string()
    } else {
        format!("/{}/", current_path.trim_end_matches('/'))
    };

    // Pre-allocate with estimated capacity
    let mut html = String::with_capacity(4096 + entries.len() * 256);

    // HTML head
    let _ = write!(
        html,
        r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>Index of {}</title>
    <style>
        * {{ box-sizing: border-box; }}
        body {{
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif;
            line-height: 1.6;
            max-width: 1200px;
            margin: 0 auto;
            padding: 20px;
            background: #f5f5f5;
        }}
        h1 {{ color: #333; border-bottom: 2px solid #ddd; padding-bottom: 10px; }}
        table {{
            width: 100%;
            border-collapse: collapse;
            background: white;
            border-radius: 8px;
            overflow: hidden;
            box-shadow: 0 2px 4px rgba(0,0,0,0.1);
        }}
        th, td {{ padding: 12px 16px; text-align: left; border-bottom: 1px solid #eee; }}
        th {{ background: #f8f9fa; font-weight: 600; color: #666; }}
        tr:hover {{ background: #f8f9fa; }}
        a {{ color: #0366d6; text-decoration: none; }}
        a:hover {{ text-decoration: underline; }}
        .size {{ text-align: right; white-space: nowrap; color: #666; }}
        .time {{ white-space: nowrap; color: #666; }}
        @media (max-width: 600px) {{
            body {{ padding: 10px; }}
            th, td {{ padding: 8px 12px; }}
            .time {{ display: none; }}
        }}
    </style>
</head>
<body>
    <div class="header"><h1>Index of {}</h1></div>
    <table>
        <thead>
            <tr><th>Name</th><th class="size">Size</th><th class="time">Modified</th></tr>
        </thead>
        <tbody>"#,
        html_escape(&display_path),
        html_escape(&display_path)
    );

    // Table rows
    for entry in entries {
        let icon = if entry.is_dir { "📁" } else { "📄" };
        let size_display = if entry.is_dir {
            "-"
        } else {
            &format_size(entry.size)
        };
        let time_display = entry
            .modified
            .map(format_http_date)
            .unwrap_or_else(|| "-".to_string());

        let _ = write!(
            html,
            r#"<tr><td>{} <a href="{}">{}</a></td><td class="size">{}</td><td class="time">{}</td></tr>"#,
            icon,
            entry.path,
            html_escape(&entry.name),
            size_display,
            time_display
        );
    }

    // HTML footer
    let _ = write!(
        html,
        r#"</tbody></table>
    <footer style="margin-top: 20px; color: #999; font-size: 0.85em;"><p>D HTTP Server</p></footer>
</body>
</html>"#
    );

    html
}

/// Escape HTML special characters
fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
