use crate::utils::{decode_path, encode_path, format_http_date, format_size, guess_mime_type};
use axum::{
    body::Body,
    extract::{Request, State},
    http::{header, HeaderMap, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::get,
    Router,
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

/// Create the router
pub fn create_app(root: PathBuf) -> Router {
    let state = ServerState { root };

    Router::new()
        .route("/{*path}", get(handle_request))
        .route("/", get(handle_root))
        .with_state(state)
}

/// Start the HTTP server
pub async fn start(addr: &SocketAddr, root: &str) {
    let root_path = PathBuf::from(root);
    
    if !root_path.exists() {
        error!("Root path does not exist: {}", root);
        return;
    }

    let app = create_app(root_path);

    info!("Starting server on http://{}", addr);
    info!("Serving directory: {}", root);

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    
    axum::serve(listener, app)
        .await
        .unwrap();
}

/// Handle root path
async fn handle_root(State(state): State<ServerState>) -> impl IntoResponse {
    handle_path("", &state).await
}

/// Handle any request
async fn handle_request(
    State(state): State<ServerState>,
    request: Request<Body>,
) -> impl IntoResponse {
    let path = request
        .uri()
        .path()
        .trim_start_matches('/');
    
    let decoded = match decode_path(path) {
        Ok(p) => p,
        Err(_) => {
            return (StatusCode::BAD_REQUEST, "Invalid encoding").into_response();
        }
    };

    if decoded == "/favicon.ico" {
        return StatusCode::NOT_FOUND.into_response();
    }

    info!("{} {}", request.method(), decoded);
    
    handle_path(&decoded, &state).await
}

/// Handle path - either serve file or directory listing
async fn handle_path(path: &str, state: &ServerState) -> Response {
    let full_path = sanitize_path(&state.root, path);
    
    match fs::metadata(&full_path).await {
        Ok(metadata) if metadata.is_dir() => {
            serve_directory(path, &full_path, state).await
        }
        Ok(metadata) => {
            serve_file(&full_path, &metadata).await
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            (StatusCode::NOT_FOUND, "Not Found").into_response()
        }
        Err(e) => {
            error!("Error accessing {}: {}", full_path.display(), e);
            (StatusCode::INTERNAL_SERVER_ERROR, "Internal Server Error").into_response()
        }
    }
}

/// Sanitize path to prevent directory traversal attacks
fn sanitize_path(base: &Path, requested: &str) -> PathBuf {
    let mut result = base.to_path_buf();
    
    for component in Path::new(requested).components() {
        match component {
            Component::Normal(c) => result.push(c),
            Component::RootDir => {}
            _ => {}
        }
    }
    
    result
}

/// Serve a file with proper streaming
async fn serve_file(path: &PathBuf, metadata: &std::fs::Metadata) -> Response {
    let file = match fs::File::open(path).await {
        Ok(f) => f,
        Err(e) => {
            error!("Failed to open file {}: {}", path.display(), e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "Internal Server Error").into_response();
        }
    };

    let mut headers = HeaderMap::new();
    
    // Set content type
    let mime = guess_mime_type(
        path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
    );
    headers.insert(header::CONTENT_TYPE, mime.parse().unwrap());
    
    // Set content length
    headers.insert(
        header::CONTENT_LENGTH,
        metadata.len().to_string().parse().unwrap(),
    );
    
    // Set accept-ranges
    headers.insert(header::ACCEPT_RANGES, "bytes".parse().unwrap());
    
    // Set last-modified
    if let Ok(modified) = metadata.modified() {
        if let Ok(time_str) = time::OffsetDateTime::from(modified)
            .format(&time::format_description::well_known::Rfc2822)
        {
            headers.insert(header::LAST_MODIFIED, time_str.parse().unwrap());
        }
    }

    // Create stream body
    let stream = ReaderStream::new(file);
    let body = Body::from_stream(stream);

    (StatusCode::OK, headers, body).into_response()
}

/// Serve directory listing HTML
async fn serve_directory(rel_path: &str, full_path: &PathBuf, _state: &ServerState) -> Response {
    let mut entries = match fs::read_dir(full_path).await {
        Ok(entries) => entries,
        Err(e) => {
            warn!("Cannot read directory {}: {}", full_path.display(), e);
            return (StatusCode::FORBIDDEN, "Permission Denied").into_response();
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
        
        let path = format!(
            "{}{}",
            encode_path(&name),
            if is_dir { "/" } else { "" }
        );
        
        files.push(FileEntry {
            name,
            path,
            is_dir,
            size,
            modified,
        });
    }

    // Sort: directories first, then by name
    files.sort_by(|a, b| {
        match (a.is_dir, b.is_dir) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        }
    });

    let html = generate_directory_html(rel_path, &files);
    Html(html).into_response()
}

/// Generate HTML for directory listing
fn generate_directory_html(current_path: &str, entries: &[FileEntry]) -> String {
    let display_path = if current_path.is_empty() {
        "/".to_string()
    } else {
        format!("/{}/", current_path.trim_end_matches('/'))
    };
    
    let title = format!("Index of {}", display_path);
    
    let mut rows = String::new();
    
    for entry in entries {
        let icon = if entry.is_dir {
            "📁"
        } else {
            "📄"
        };
        
        let size_display = if entry.is_dir {
            "-".to_string()
        } else {
            format_size(entry.size)
        };
        
        let time_display = entry
            .modified
            .map(format_http_date)
            .unwrap_or_else(|| "-".to_string());
        
        rows.push_str(&format!(
            r#"<tr>
                <td>{icon} <a href="{href}">{name}</a></td>
                <td class="size">{size}</td>
                <td class="time">{time}</td>
            </tr>"#,
            icon = icon,
            href = entry.path,
            name = html_escape(&entry.name),
            size = size_display,
            time = time_display,
        ));
    }

    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>{title}</title>
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
        h1 {{
            color: #333;
            border-bottom: 2px solid #ddd;
            padding-bottom: 10px;
        }}
        table {{
            width: 100%;
            border-collapse: collapse;
            background: white;
            border-radius: 8px;
            overflow: hidden;
            box-shadow: 0 2px 4px rgba(0,0,0,0.1);
        }}
        th, td {{
            padding: 12px 16px;
            text-align: left;
            border-bottom: 1px solid #eee;
        }}
        th {{
            background: #f8f9fa;
            font-weight: 600;
            color: #666;
        }}
        tr:hover {{ background: #f8f9fa; }}
        a {{
            color: #0366d6;
            text-decoration: none;
        }}
        a:hover {{ text-decoration: underline; }}
        .size {{
            text-align: right;
            white-space: nowrap;
            color: #666;
        }}
        .time {{
            white-space: nowrap;
            color: #666;
        }}
        .header {{
            margin-bottom: 20px;
        }}
        @media (max-width: 600px) {{
            body {{ padding: 10px; }}
            th, td {{ padding: 8px 12px; }}
            .time {{ display: none; }}
        }}
    </style>
</head>
<body>
    <div class="header">
        <h1>{title}</h1>
    </div>
    <table>
        <thead>
            <tr>
                <th>Name</th>
                <th class="size">Size</th>
                <th class="time">Modified</th>
            </tr>
        </thead>
        <tbody>
            {rows}
        </tbody>
    </table>
    <footer style="margin-top: 20px; color: #999; font-size: 0.85em;">
        <p>D HTTP Server</p>
    </footer>
</body>
</html>"#,
        title = title,
        rows = rows
    )
}

/// Escape HTML special characters
fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
