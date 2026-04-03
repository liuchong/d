use crate::utils::{
    decode_path, encode_path, format_http_date, format_size, guess_mime_type,
};
use axum::{
    Router,
    body::Body,
    extract::{Query, Request, State},
    http::{HeaderMap, Method, StatusCode, header},
    response::{Html, IntoResponse, Response},
    routing::get,
};
use serde::Deserialize;
use std::net::SocketAddr;
use std::path::{Component, Path, PathBuf};

use tokio::fs;
use tokio_util::io::ReaderStream;
use tracing::{error, info, warn};

/// Server state
#[derive(Clone)]
pub struct ServerState {
    root: PathBuf,
    allow_hidden: bool,
}

/// File information for directory listing
struct FileEntry {
    name: String,
    path: String,
    is_dir: bool,
    size: u64,
    modified_time: Option<std::time::SystemTime>,
    file_type: FileType,
    extension: String,
}

#[derive(Debug, Clone)]
enum FileType {
    Directory,
    Image,
    Video,
    Audio,
    Code,
    Text,
    Markdown,
    Org,
    Archive,
    Document,
    Executable,
    Unknown,
}

impl FileType {
    fn from_extension(ext: &str) -> Self {
        match ext.to_lowercase().as_str() {
            "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp" | "svg" | "ico"
            | "tiff" | "tif" | "raw" | "heic" | "avif" => FileType::Image,
            "mp4" | "webm" | "mkv" | "avi" | "mov" | "wmv" | "flv" | "m4v"
            | "3gp" | "ogv" | "mpg" | "mpeg" | "m2v" => FileType::Video,
            "mp3" | "wav" | "ogg" | "flac" | "aac" | "m4a" | "wma" | "opus"
            | "aiff" | "au" => FileType::Audio,
            "rs" | "js" | "ts" | "jsx" | "tsx" | "py" | "java" | "c"
            | "cpp" | "cc" | "cxx" | "h" | "hpp" | "go" | "rb" | "php"
            | "swift" | "kt" | "scala" | "r" | "m" | "mm" | "pl" | "pm"
            | "t" | "sh" | "bash" | "zsh" | "fish" | "ps1" | "bat" | "cmd"
            | "vbs" | "lua" | "elm" | "erl" | "hrl" | "ex" | "exs" | "fs"
            | "fsx" | "fsi" | "ml" | "mli" | "hs" | "lhs" | "clj" | "cljs"
            | "cljc" | "edn" | "coffee" | "litcoffee" | "cr" | "dart"
            | "groovy" | "gvy" | "gy" | "gsh" | "p6" | "pm6" | "pod6"
            | "t6" | "nim" | "nims" | "zig" | "v" | "vsh" => FileType::Code,
            "txt" | "rst" | "log" | "csv" | "tsv" | "json" | "xml" | "yaml"
            | "yml" | "toml" | "ini" | "conf" | "cfg" | "properties"
            | "env" | "sql" | "graphql" | "gql" => FileType::Text,
            "md" | "markdown" | "mkd" | "mdown" => FileType::Markdown,
            "org" => FileType::Org,
            "zip" | "rar" | "7z" | "tar" | "gz" | "bz2" | "xz" | "lz"
            | "lzma" | "zst" | "br" | "tgz" | "tbz" | "txz" | "tlz" | "cab"
            | "deb" | "rpm" | "dmg" | "pkg" | "msi" | "iso" | "img" => {
                FileType::Archive
            }
            "pdf" | "doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx"
            | "odt" | "ods" | "odp" | "rtf" | "epub" | "mobi" | "azw"
            | "azw3" | "tex" | "latex" => FileType::Document,
            "exe" | "dll" | "so" | "dylib" | "bin" | "app" | "elf" | "wasm"
            | "pyc" | "class" | "o" | "obj" | "lib" | "a" => {
                FileType::Executable
            }
            _ => FileType::Unknown,
        }
    }

    fn icon(&self) -> &'static str {
        match self {
            FileType::Directory => "📁",
            FileType::Image => "🖼️",
            FileType::Video => "🎬",
            FileType::Audio => "🎵",
            FileType::Code => "📄",
            FileType::Text => "📝",
            FileType::Markdown => "📜",
            FileType::Org => "📋",
            FileType::Archive => "📦",
            FileType::Document => "📑",
            FileType::Executable => "⚙️",
            FileType::Unknown => "📄",
        }
    }

    fn color(&self) -> &'static str {
        match self {
            FileType::Directory => "#2196f3",
            FileType::Image => "#e91e63",
            FileType::Video => "#f44336",
            FileType::Audio => "#9c27b0",
            FileType::Code => "#4caf50",
            FileType::Text => "#607d8b",
            FileType::Markdown => "#03a9f4",
            FileType::Org => "#00bcd4",
            FileType::Archive => "#ff9800",
            FileType::Document => "#3f51b5",
            FileType::Executable => "#795548",
            FileType::Unknown => "#9e9e9e",
        }
    }

    fn is_text(&self) -> bool {
        matches!(
            self,
            FileType::Code
                | FileType::Text
                | FileType::Markdown
                | FileType::Org
        )
    }
}

#[derive(Deserialize)]
struct ViewQuery {
    view: Option<String>,
}

#[derive(Deserialize, Debug, Clone, Copy)]
#[serde(rename_all = "lowercase")]
enum SortBy {
    Name,
    Size,
    Time,
    Type,
}

impl Default for SortBy {
    fn default() -> Self {
        SortBy::Name
    }
}

#[derive(Deserialize, Clone)]
struct DirQuery {
    #[serde(default)]
    sort: SortBy,
    hidden: Option<bool>,
}

/// Create the router
pub fn create_app(root: PathBuf, allow_hidden: bool) -> Router {
    use tower_http::compression::CompressionLayer;
    use tower_http::cors::CorsLayer;

    let state = ServerState { root, allow_hidden };

    let cors = CorsLayer::new()
        .allow_origin(tower_http::cors::Any)
        .allow_methods([axum::http::Method::GET, axum::http::Method::HEAD])
        .allow_headers(tower_http::cors::Any);

    let compression = CompressionLayer::new().gzip(true).deflate(true).br(true);

    Router::new()
        .route("/{*path}", get(handle_request))
        .route("/", get(handle_root))
        .layer(compression)
        .layer(cors)
        .with_state(state)
}

pub async fn start(addr: &SocketAddr, root: &str, allow_hidden: bool) {
    let root_path = PathBuf::from(root);

    if !root_path.exists() {
        error!("Root path does not exist: {}", root);
        return;
    }

    let app = create_app(root_path, allow_hidden);

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

async fn handle_root(
    State(state): State<ServerState>,
    Query(query): Query<DirQuery>,
) -> impl IntoResponse {
    handle_dir("", query, &state).await
}

async fn handle_request(
    State(state): State<ServerState>,
    Query(dir_query): Query<DirQuery>,
    Query(view_query): Query<ViewQuery>,
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

    // Check if this is a view request for text files
    if let Some(view) = view_query.view {
        return handle_view(&decoded, &view, &state).await;
    }

    handle_path(&decoded, range, is_head, dir_query, &state).await
}

async fn handle_view(path: &str, view: &str, state: &ServerState) -> Response {
    let full_path = match sanitize_path(&state.root, path) {
        Some(p) => p,
        None => {
            return (StatusCode::FORBIDDEN, "Invalid path").into_response();
        }
    };

    let metadata = match fs::metadata(&full_path).await {
        Ok(m) if m.is_file() => m,
        Ok(_) => {
            return (StatusCode::BAD_REQUEST, "Not a file").into_response();
        }
        Err(_) => return (StatusCode::NOT_FOUND, "Not Found").into_response(),
    };

    let ext = Path::new(path)
        .extension()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let file_type = FileType::from_extension(&ext);

    match view.as_ref() {
        "raw" => serve_raw_file(&full_path, &metadata).await,
        "preview" => serve_preview(&full_path, path, &file_type).await,
        "download" => serve_download(&full_path, path, &metadata).await,
        _ => (StatusCode::BAD_REQUEST, "Invalid view").into_response(),
    }
}

async fn handle_path(
    path: &str,
    range: Option<&str>,
    is_head: bool,
    dir_query: DirQuery,
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
                (StatusCode::OK, Html("")).into_response()
            } else {
                serve_directory(path, &full_path, dir_query, state).await
            }
        }
        Ok(metadata) => {
            // Check if it's a text file that should be shown with options
            let ext = Path::new(path)
                .extension()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let file_type = FileType::from_extension(&ext);

            if file_type.is_text()
                || matches!(file_type, FileType::Markdown | FileType::Org)
            {
                // For text files, show the file viewer page
                serve_file_viewer(path, &full_path, &metadata, &file_type).await
            } else {
                serve_file(&full_path, &metadata, range, is_head).await
            }
        }
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

async fn handle_dir(
    path: &str,
    query: DirQuery,
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
            serve_directory(path, &full_path, query, state).await
        }
        _ => (StatusCode::NOT_FOUND, "Not Found").into_response(),
    }
}

fn sanitize_path(base: &Path, requested: &str) -> Option<PathBuf> {
    let mut result = base.to_path_buf();

    for component in Path::new(requested).components() {
        match component {
            Component::Normal(c) => result.push(c),
            Component::ParentDir => {
                if result != *base {
                    result.pop();
                }
            }
            Component::RootDir | Component::CurDir => {}
            Component::Prefix(_) => return None,
        }
    }

    if result.starts_with(base) {
        Some(result)
    } else {
        None
    }
}

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

fn generate_etag(metadata: &std::fs::Metadata) -> String {
    let modified = metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("\"{:x}-{:x}\"", modified, metadata.len())
}

async fn serve_file(
    path: &PathBuf,
    metadata: &std::fs::Metadata,
    range_header: Option<&str>,
    is_head: bool,
) -> Response {
    let file_size = metadata.len();
    let etag = generate_etag(metadata);

    let mut headers = HeaderMap::new();

    let mime = guess_mime_type(
        path.file_name().and_then(|n| n.to_str()).unwrap_or(""),
    );
    headers.insert(header::CONTENT_TYPE, mime.parse().unwrap());
    headers.insert(header::ACCEPT_RANGES, "bytes".parse().unwrap());
    headers.insert(
        header::CACHE_CONTROL,
        "public, max-age=3600".parse().unwrap(),
    );
    headers.insert(header::ETAG, etag.parse().unwrap());

    if let Ok(modified) = metadata.modified() {
        if let Ok(time_str) = time::OffsetDateTime::from(modified)
            .format(&time::format_description::well_known::Rfc2822)
        {
            headers.insert(header::LAST_MODIFIED, time_str.parse().unwrap());
        }
    }

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

        if is_head {
            return (StatusCode::PARTIAL_CONTENT, headers).into_response();
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

        return (StatusCode::PARTIAL_CONTENT, headers, body).into_response();
    }

    headers.insert(
        header::CONTENT_LENGTH,
        file_size.to_string().parse().unwrap(),
    );

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

async fn serve_raw_file(
    path: &PathBuf,
    metadata: &std::fs::Metadata,
) -> Response {
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

    let mut headers = HeaderMap::new();
    let mime = guess_mime_type(
        path.file_name().and_then(|n| n.to_str()).unwrap_or(""),
    );
    headers.insert(header::CONTENT_TYPE, mime.parse().unwrap());
    headers.insert(
        header::CONTENT_LENGTH,
        metadata.len().to_string().parse().unwrap(),
    );

    let stream = ReaderStream::new(file);
    let body = Body::from_stream(stream);

    (StatusCode::OK, headers, body).into_response()
}

async fn serve_download(
    path: &PathBuf,
    filename: &str,
    metadata: &std::fs::Metadata,
) -> Response {
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

    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_DISPOSITION,
        format!("attachment; filename=\"{}\"", filename)
            .parse()
            .unwrap(),
    );
    headers.insert(
        header::CONTENT_LENGTH,
        metadata.len().to_string().parse().unwrap(),
    );

    let stream = ReaderStream::new(file);
    let body = Body::from_stream(stream);

    (StatusCode::OK, headers, body).into_response()
}

async fn serve_preview(
    path: &PathBuf,
    relative_path: &str,
    file_type: &FileType,
) -> Response {
    let content = match fs::read_to_string(path).await {
        Ok(c) => c,
        Err(_) => {
            // Binary file, fallback to raw
            return serve_raw_file(path, &std::fs::metadata(path).unwrap())
                .await;
        }
    };

    let html = match file_type {
        FileType::Markdown => render_markdown(relative_path, &content),
        FileType::Org => render_org(relative_path, &content),
        FileType::Code => render_code(relative_path, &content, file_type),
        FileType::Text => render_text(relative_path, &content),
        _ => render_text(relative_path, &content),
    };

    Html(html).into_response()
}

async fn serve_file_viewer(
    relative_path: &str,
    full_path: &PathBuf,
    metadata: &std::fs::Metadata,
    file_type: &FileType,
) -> Response {
    let content = match fs::read_to_string(full_path).await {
        Ok(c) => c,
        Err(_) => {
            // Binary file, serve directly
            return serve_file(full_path, metadata, None, false).await;
        }
    };

    let file_name = Path::new(relative_path)
        .file_name()
        .unwrap_or_default()
        .to_string_lossy();

    let size = format_size(metadata.len());
    let modified = metadata
        .modified()
        .ok()
        .map(format_http_date)
        .unwrap_or_default();

    let file_type_name = format!("{:?}", file_type);
    let file_type_icon = file_type.icon();
    let file_type_color = file_type.color();

    // Determine default preview content
    let preview_content = match file_type {
        FileType::Markdown => render_markdown_content(&content),
        FileType::Org => render_org_content(&content),
        FileType::Code => render_code_content(&content, full_path),
        FileType::Text => render_text_content(&content),
        _ => html_escape(&content),
    };

    let html = format!(
        r##"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>{}</title>
    <link rel="stylesheet" href="https://cdnjs.cloudflare.com/ajax/libs/highlight.js/11.9.0/styles/github-dark.min.css">
    <script src="https://cdnjs.cloudflare.com/ajax/libs/highlight.js/11.9.0/highlight.min.js"></script>
    <script src="https://cdnjs.cloudflare.com/ajax/libs/highlight.js/11.9.0/languages/rust.min.js"></script>
    <script src="https://cdnjs.cloudflare.com/ajax/libs/highlight.js/11.9.0/languages/javascript.min.js"></script>
    <script src="https://cdnjs.cloudflare.com/ajax/libs/highlight.js/11.9.0/languages/python.min.js"></script>
    <script src="https://cdnjs.cloudflare.com/ajax/libs/highlight.js/11.9.0/languages/go.min.js"></script>
    <script src="https://cdnjs.cloudflare.com/ajax/libs/highlight.js/11.9.0/languages/bash.min.js"></script>
    <script src="https://cdnjs.cloudflare.com/ajax/libs/highlight.js/11.9.0/languages/json.min.js"></script>
    <script src="https://cdnjs.cloudflare.com/ajax/libs/highlight.js/11.9.0/languages/yaml.min.js"></script>
    <script src="https://cdn.jsdelivr.net/npm/marked/marked.min.js"></script>
    <style>
        * {{ box-sizing: border-box; margin: 0; padding: 0; }}
        body {{
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif;
            line-height: 1.6;
            background: #0d1117;
            color: #c9d1d9;
            min-height: 100vh;
        }}
        .header {{
            background: #161b22;
            border-bottom: 1px solid #30363d;
            padding: 16px 24px;
            position: sticky;
            top: 0;
            z-index: 100;
        }}
        .file-info {{
            display: flex;
            align-items: center;
            gap: 12px;
            margin-bottom: 12px;
        }}
        .file-icon {{ font-size: 2rem; }}
        .file-details {{ flex: 1; }}
        .file-name {{
            font-size: 1.25rem;
            font-weight: 600;
            color: #f0f6fc;
            word-break: break-all;
        }}
        .file-meta {{
            font-size: 0.85rem;
            color: #8b949e;
            margin-top: 4px;
        }}
        .file-type-badge {{
            display: inline-flex;
            align-items: center;
            padding: 4px 10px;
            border-radius: 20px;
            font-size: 0.75rem;
            font-weight: 600;
            text-transform: uppercase;
            background: {};
            color: #fff;
        }}
        .actions {{
            display: flex;
            gap: 8px;
            flex-wrap: wrap;
        }}
        .btn {{
            display: inline-flex;
            align-items: center;
            gap: 6px;
            padding: 8px 16px;
            border-radius: 6px;
            font-size: 0.9rem;
            font-weight: 500;
            text-decoration: none;
            cursor: pointer;
            border: 1px solid #30363d;
            background: #21262d;
            color: #c9d1d9;
            transition: all 0.2s;
        }}
        .btn:hover {{
            background: #30363d;
            border-color: #8b949e;
        }}
        .btn.active {{
            background: #1f6feb;
            border-color: #1f6feb;
            color: #fff;
        }}
        .btn-secondary {{
            background: transparent;
        }}
        .content {{
            max-width: 1200px;
            margin: 0 auto;
            padding: 24px;
        }}
        .preview-container {{
            background: #161b22;
            border: 1px solid #30363d;
            border-radius: 12px;
            overflow: hidden;
        }}
        .preview-header {{
            background: #21262d;
            padding: 12px 16px;
            font-size: 0.85rem;
            color: #8b949e;
            border-bottom: 1px solid #30363d;
        }}
        .preview-content {{
            padding: 24px;
            overflow-x: auto;
        }}
        /* Markdown styles */
        .markdown-body {{
            line-height: 1.8;
        }}
        .markdown-body h1, .markdown-body h2, .markdown-body h3,
        .markdown-body h4, .markdown-body h5, .markdown-body h6 {{
            margin-top: 24px;
            margin-bottom: 16px;
            font-weight: 600;
            line-height: 1.25;
            color: #f0f6fc;
        }}
        .markdown-body h1 {{ font-size: 2em; border-bottom: 1px solid #30363d; padding-bottom: 10px; }}
        .markdown-body h2 {{ font-size: 1.5em; border-bottom: 1px solid #30363d; padding-bottom: 8px; }}
        .markdown-body p {{ margin-bottom: 16px; }}
        .markdown-body code {{
            background: #0d1117;
            padding: 2px 6px;
            border-radius: 4px;
            font-family: 'SF Mono', Monaco, monospace;
            font-size: 0.9em;
        }}
        .markdown-body pre {{
            background: #0d1117;
            padding: 16px;
            border-radius: 8px;
            overflow-x: auto;
            margin-bottom: 16px;
        }}
        .markdown-body pre code {{ background: none; padding: 0; }}
        .markdown-body ul, .markdown-body ol {{
            margin-bottom: 16px;
            padding-left: 2em;
        }}
        .markdown-body li {{ margin-bottom: 4px; }}
        .markdown-body a {{ color: #58a6ff; }}
        .markdown-body a:hover {{ text-decoration: underline; }}
        .markdown-body blockquote {{
            border-left: 4px solid #30363d;
            padding-left: 16px;
            margin-left: 0;
            color: #8b949e;
        }}
        .markdown-body table {{
            border-collapse: collapse;
            margin-bottom: 16px;
            width: 100%;
        }}
        .markdown-body th, .markdown-body td {{
            border: 1px solid #30363d;
            padding: 8px 12px;
            text-align: left;
        }}
        .markdown-body th {{ background: #0d1117; font-weight: 600; }}
        .markdown-body img {{ max-width: 100%; border-radius: 8px; }}
        /* Code styles */
        .code-block {{
            margin: 0;
            font-size: 0.9rem;
            line-height: 1.6;
        }}
        .code-block pre {{
            margin: 0;
            padding: 20px;
            overflow-x: auto;
        }}
        /* Org mode styles */
        .org-body {{
            font-family: 'SF Mono', Monaco, monospace;
            font-size: 0.9rem;
            line-height: 1.8;
            white-space: pre-wrap;
        }}
        .org-body .org-heading {{
            color: #7ee787;
            font-weight: 600;
        }}
        .org-body .org-todo {{ color: #ffa198; font-weight: 600; }}
        .org-body .org-done {{ color: #7ee787; }}
        .org-body .org-tag {{ color: #79c0ff; }}
        .org-body .org-source {{
            background: #0d1117;
            padding: 16px;
            border-radius: 8px;
            margin: 8px 0;
        }}
        .back-link {{
            display: inline-flex;
            align-items: center;
            gap: 6px;
            color: #58a6ff;
            text-decoration: none;
            margin-bottom: 16px;
            font-size: 0.9rem;
        }}
        .back-link:hover {{ text-decoration: underline; }}
    </style>
</head>
<body>
    <div class="header">
        <a href="./" class="back-link">← Back to directory</a>
        <div class="file-info">
            <span class="file-icon">{}</span>
            <div class="file-details">
                <div class="file-name">{}</div>
                <div class="file-meta">{} · {}</div>
            </div>
            <span class="file-type-badge">{}</span>
        </div>
        <div class="actions">
            <button class="btn active" onclick="showTab('preview')">📄 Preview</button>
            <button class="btn btn-secondary" onclick="showTab('raw')">📝 Raw</button>
            <a href="?view=download" class="btn btn-secondary">⬇️ Download</a>
        </div>
    </div>

    <div class="content">
        <div id="preview-tab" class="preview-container">
            <div class="preview-header">Preview</div>
            <div class="preview-content">
                {}
            </div>
        </div>

        <div id="raw-tab" class="preview-container" style="display: none;">
            <div class="preview-header">Raw</div>
            <div class="preview-content">
                <pre class="code-block"><code id="raw-content">{}</code></pre>
            </div>
        </div>
    </div>

    <script>
        function showTab(tab) {{
            document.getElementById('preview-tab').style.display = tab === 'preview' ? 'block' : 'none';
            document.getElementById('raw-tab').style.display = tab === 'raw' ? 'block' : 'none';
            document.querySelectorAll('.btn').forEach(b => b.classList.remove('active'));
            event.target.classList.add('active');
        }}

        // Highlight code blocks
        document.querySelectorAll('pre code').forEach((block) => {{
            hljs.highlightElement(block);
        }});
    </script>
</body>
</html>"##,
        html_escape(&file_name),
        file_type_color,
        file_type_icon,
        html_escape(&file_name),
        size,
        modified,
        file_type_name,
        preview_content,
        html_escape(&content)
    );

    Html(html).into_response()
}

fn render_markdown(_path: &str, content: &str) -> String {
    render_markdown_content(content)
}

fn render_markdown_content(content: &str) -> String {
    let escaped = html_escape(content);
    format!(
        r#"<div class="markdown-body" id="markdown-content">{}</div>
<script>
    document.getElementById('markdown-content').innerHTML = marked.parse(`{}`);
</script>"#,
        escaped.replace('`', "\\`"),
        escaped.replace('`', "\\`").replace('$', "\\$")
    )
}

fn render_org(_path: &str, content: &str) -> String {
    render_org_content(content)
}

fn render_org_content(content: &str) -> String {
    let mut html = String::new();
    html.push_str("<div class=\"org-body\">");

    for line in content.lines() {
        let processed = if line.starts_with("* ") {
            format!("<div class=\"org-heading\">{}</div>", html_escape(line))
        } else if line.starts_with("#+BEGIN_SRC")
            || line.starts_with("#+END_SRC")
        {
            format!("<div class=\"org-source\">{}</div>", html_escape(line))
        } else if line.starts_with("#+") {
            format!(
                "<div style=\"color: #8b949e;\">{}</div>",
                html_escape(line)
            )
        } else if line.contains("TODO") {
            line.replace("TODO", "<span class=\"org-todo\">TODO</span>")
                .replace("DONE", "<span class=\"org-done\">DONE</span>")
        } else {
            html_escape(line)
        };
        html.push_str(&processed);
        html.push('\n');
    }

    html.push_str("</div>");
    html
}

fn render_code(_path: &str, content: &str, _file_type: &FileType) -> String {
    render_code_content(content, &std::path::PathBuf::from(_path))
}

fn render_code_content(content: &str, path: &Path) -> String {
    let ext = path.extension().unwrap_or_default().to_string_lossy();
    let lang = get_language(&ext);

    format!(
        r#"<pre class="code-block"><code class="language-{}">{}</code></pre>
<script>hljs.highlightAll();</script>"#,
        lang,
        html_escape(content)
    )
}

fn render_text(_path: &str, content: &str) -> String {
    render_text_content(content)
}

fn render_text_content(content: &str) -> String {
    format!(
        "<pre class=\"code-block\"><code>{}</code></pre>",
        html_escape(content)
    )
}

fn get_language(ext: &str) -> &'static str {
    match ext.to_lowercase().as_str() {
        "rs" => "rust",
        "js" => "javascript",
        "ts" => "typescript",
        "jsx" => "jsx",
        "tsx" => "tsx",
        "py" => "python",
        "java" => "java",
        "c" => "c",
        "cpp" | "cc" | "cxx" | "hpp" => "cpp",
        "h" => "c",
        "go" => "go",
        "rb" => "ruby",
        "php" => "php",
        "swift" => "swift",
        "kt" => "kotlin",
        "scala" => "scala",
        "r" => "r",
        "sh" | "bash" | "zsh" => "bash",
        "ps1" => "powershell",
        "lua" => "lua",
        "elm" => "elm",
        "erl" | "hrl" => "erlang",
        "ex" | "exs" => "elixir",
        "fs" | "fsx" => "fsharp",
        "ml" | "mli" => "ocaml",
        "hs" | "lhs" => "haskell",
        "clj" | "cljs" => "clojure",
        "coffee" => "coffeescript",
        "cr" => "crystal",
        "dart" => "dart",
        "groovy" => "groovy",
        "nim" => "nim",
        "zig" => "zig",
        "v" => "v",
        "sql" => "sql",
        "json" => "json",
        "xml" => "xml",
        "yaml" | "yml" => "yaml",
        "toml" => "toml",
        "md" | "markdown" => "markdown",
        "html" | "htm" => "html",
        "css" => "css",
        _ => "plaintext",
    }
}

async fn serve_directory(
    rel_path: &str,
    full_path: &PathBuf,
    query: DirQuery,
    state: &ServerState,
) -> Response {
    let entries =
        match list_directory_entries(rel_path, full_path, query.clone(), state)
            .await
        {
            Ok(e) => e,
            Err(e) => {
                warn!("Cannot read directory {}: {}", full_path.display(), e);
                return (StatusCode::FORBIDDEN, "Permission Denied")
                    .into_response();
            }
        };

    let html = generate_directory_html(rel_path, &entries, query, state);
    Html(html).into_response()
}

async fn list_directory_entries(
    rel_path: &str,
    full_path: &PathBuf,
    query: DirQuery,
    _state: &ServerState,
) -> std::io::Result<Vec<FileEntry>> {
    let mut entries = fs::read_dir(full_path).await?;
    let mut files = Vec::new();

    // Determine if we should show hidden files
    let show_hidden = if _state.allow_hidden {
        query.hidden.unwrap_or(true)
    } else {
        false
    };

    if !rel_path.is_empty() {
        files.push(FileEntry {
            name: "..".to_string(),
            path: "../".to_string(),
            is_dir: true,
            size: 0,
            modified_time: None,
            file_type: FileType::Directory,
            extension: "".to_string(),
        });
    }

    while let Some(entry) = entries.next_entry().await? {
        let name = entry.file_name().to_string_lossy().to_string();

        // Skip hidden files if not showing them
        if !show_hidden && name.starts_with('.') {
            continue;
        }

        let metadata = entry.metadata().await.ok();

        let is_dir = metadata.as_ref().map(|m| m.is_dir()).unwrap_or(false);
        let size = metadata.as_ref().map(|m| m.len()).unwrap_or(0);
        let modified_time = metadata.and_then(|m| m.modified().ok());

        let extension = Path::new(&name)
            .extension()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        let file_type = if is_dir {
            FileType::Directory
        } else {
            FileType::from_extension(&extension)
        };

        let path =
            format!("{}{}", encode_path(&name), if is_dir { "/" } else { "" });

        files.push(FileEntry {
            name,
            path,
            is_dir,
            size,
            modified_time,
            file_type,
            extension,
        });
    }

    // Sort based on query parameter
    match query.sort {
        SortBy::Name => {
            files.sort_by(|a, b| match (a.is_dir, b.is_dir) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            });
        }
        SortBy::Size => {
            files.sort_by(|a, b| match (a.is_dir, b.is_dir) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => a.size.cmp(&b.size),
            });
        }
        SortBy::Time => {
            files.sort_by(|a, b| {
                match (a.is_dir, b.is_dir) {
                    (true, false) => std::cmp::Ordering::Less,
                    (false, true) => std::cmp::Ordering::Greater,
                    _ => b.modified_time.cmp(&a.modified_time), // Newest first
                }
            });
        }
        SortBy::Type => {
            files.sort_by(|a, b| match (a.is_dir, b.is_dir) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => {
                    let type_order = |ft: &FileType| match ft {
                        FileType::Directory => 0,
                        FileType::Code => 1,
                        FileType::Text => 2,
                        FileType::Markdown => 3,
                        FileType::Org => 4,
                        FileType::Image => 5,
                        FileType::Video => 6,
                        FileType::Audio => 7,
                        FileType::Document => 8,
                        FileType::Archive => 9,
                        FileType::Executable => 10,
                        FileType::Unknown => 11,
                    };
                    type_order(&a.file_type).cmp(&type_order(&b.file_type))
                }
            });
        }
    }

    Ok(files)
}

fn generate_breadcrumb(current_path: &str) -> String {
    if current_path.is_empty() {
        return "<a href=\"/\">/</a>".to_string();
    }

    let mut result = String::new();
    result.push_str("<a href=\"/\">/</a>");

    let parts: Vec<&str> =
        current_path.split('/').filter(|s| !s.is_empty()).collect();
    let mut cumulative_path = String::new();

    for (i, part) in parts.iter().enumerate() {
        cumulative_path.push('/');
        cumulative_path.push_str(part);

        if i == parts.len() - 1 {
            // Last part - current directory, not a link
            result.push_str(&format!("<span>{}</span>", html_escape(part)));
        } else {
            // Parent directory, make it a link
            result.push_str(&format!(
                "<a href=\"{}\">{}</a>/",
                encode_path(&cumulative_path),
                html_escape(part)
            ));
        }
    }

    result
}

fn generate_directory_html(
    current_path: &str,
    entries: &[FileEntry],
    query: DirQuery,
    state: &ServerState,
) -> String {
    use std::fmt::Write;

    let display_path = if current_path.is_empty() {
        "/".to_string()
    } else {
        format!("/{}/", current_path.trim_end_matches('/'))
    };

    let breadcrumb = generate_breadcrumb(current_path);

    // Determine current sort and hidden settings
    let current_sort = query.sort;
    let show_hidden = query.hidden.unwrap_or(true);

    // Build sort links (preserve other query params)
    let sort_link = |sort: SortBy| {
        let sort_name = match sort {
            SortBy::Name => "name",
            SortBy::Size => "size",
            SortBy::Time => "time",
            SortBy::Type => "type",
        };
        let hidden_param = if show_hidden {
            "&hidden=true"
        } else {
            "&hidden=false"
        };
        format!("?sort={}{}", sort_name, hidden_param)
    };

    // Build hidden toggle link
    let hidden_toggle_link = if state.allow_hidden {
        let sort_name = match current_sort {
            SortBy::Name => "name",
            SortBy::Size => "size",
            SortBy::Time => "time",
            SortBy::Type => "type",
        };
        let new_hidden = !show_hidden;
        format!("?sort={}&hidden={}", sort_name, new_hidden)
    } else {
        String::new()
    };

    let mut html = String::with_capacity(4096 + entries.len() * 256);

    let _ = write!(
        html,
        r##"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>Index of {}</title>
    <style>
        * {{ box-sizing: border-box; margin: 0; padding: 0; }}
        body {{
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif;
            line-height: 1.6;
            background: #0d1117;
            color: #c9d1d9;
            min-height: 100vh;
        }}
        .container {{ max-width: 1200px; margin: 0 auto; padding: 20px; }}
        header {{
            background: #161b22;
            border-bottom: 1px solid #30363d;
            padding: 20px;
            margin: -20px -20px 20px -20px;
        }}
        h1 {{ color: #f0f6fc; font-size: 1.5rem; font-weight: 600; margin-bottom: 12px; }}
        .breadcrumb {{ color: #8b949e; font-size: 0.95rem; }}
        .breadcrumb a {{ color: #58a6ff; text-decoration: none; }}
        .breadcrumb a:hover {{ text-decoration: underline; }}
        .breadcrumb span {{ color: #f0f6fc; font-weight: 500; }}
        .controls {{
            display: flex;
            gap: 20px;
            flex-wrap: wrap;
            align-items: center;
            margin-top: 16px;
            padding-top: 16px;
            border-top: 1px solid #30363d;
        }}
        .control-group {{ display: flex; align-items: center; gap: 8px; }}
        .control-label {{ color: #8b949e; font-size: 0.85rem; }}
        .btn-group {{ display: flex; gap: 4px; }}
        .btn {{
            padding: 4px 12px;
            border-radius: 6px;
            font-size: 0.85rem;
            text-decoration: none;
            border: 1px solid #30363d;
            background: #21262d;
            color: #c9d1d9;
            cursor: pointer;
        }}
        .btn:hover {{ background: #30363d; }}
        .btn.active {{ background: #1f6feb; border-color: #1f6feb; color: #fff; }}
        .btn.disabled {{ opacity: 0.5; cursor: not-allowed; }}
        .file-list {{
            background: #161b22;
            border: 1px solid #30363d;
            border-radius: 12px;
            overflow: hidden;
        }}
        .file-header {{
            display: grid;
            grid-template-columns: auto 1fr 100px 180px;
            gap: 16px;
            padding: 12px 20px;
            background: #21262d;
            border-bottom: 1px solid #30363d;
            font-size: 0.75rem;
            font-weight: 600;
            color: #8b949e;
            text-transform: uppercase;
            letter-spacing: 0.5px;
        }}
        .file-item {{
            display: grid;
            grid-template-columns: auto 1fr 100px 180px;
            gap: 16px;
            padding: 10px 20px;
            border-bottom: 1px solid #21262d;
            align-items: center;
        }}
        .file-item:hover {{ background: #1f242c; }}
        .file-item:last-child {{ border-bottom: none; }}
        .icon {{ font-size: 1.2rem; width: 24px; text-align: center; }}
        .filename {{ display: flex; align-items: center; gap: 10px; }}
        .filename a {{
            color: #c9d1d9;
            text-decoration: none;
            font-weight: 500;
            display: flex;
            align-items: center;
            gap: 8px;
        }}
        .filename a:hover {{ color: #58a6ff; }}
        .filename .dir {{ color: #58a6ff; }}
        .file-type-badge {{
            font-size: 0.65rem;
            padding: 2px 6px;
            border-radius: 4px;
            background: #30363d;
            color: #8b949e;
            text-transform: uppercase;
            font-weight: 600;
        }}
        .size {{ text-align: right; color: #8b949e; font-family: monospace; font-size: 0.9rem; }}
        .time {{ color: #8b949e; font-size: 0.85rem; }}
        footer {{
            margin-top: 40px;
            padding: 20px;
            text-align: center;
            color: #484f58;
            font-size: 0.85rem;
        }}
    </style>
</head>
<body>
    <div class="container">
        <header>
            <h1>📁 Index of <span class="breadcrumb">{}</span></h1>
            <div class="breadcrumb">{}</div>
            <div class="controls">
                <div class="control-group">
                    <span class="control-label">Sort by:</span>
                    <div class="btn-group">
                        <a href="{}" class="btn {}">Name</a>
                        <a href="{}" class="btn {}">Size</a>
                        <a href="{}" class="btn {}">Time</a>
                        <a href="{}" class="btn {}">Type</a>
                    </div>
                </div>"##,
        html_escape(&display_path),
        html_escape(&display_path),
        breadcrumb,
        sort_link(SortBy::Name),
        if matches!(current_sort, SortBy::Name) {
            "active"
        } else {
            ""
        },
        sort_link(SortBy::Size),
        if matches!(current_sort, SortBy::Size) {
            "active"
        } else {
            ""
        },
        sort_link(SortBy::Time),
        if matches!(current_sort, SortBy::Time) {
            "active"
        } else {
            ""
        },
        sort_link(SortBy::Type),
        if matches!(current_sort, SortBy::Type) {
            "active"
        } else {
            ""
        }
    );

    // Add hidden files toggle if allowed
    if state.allow_hidden {
        let hidden_btn_class = if show_hidden { "active" } else { "" };
        let hidden_text = if show_hidden {
            "Hide hidden"
        } else {
            "Show hidden"
        };
        let _ = write!(
            html,
            r#"
                <div class="control-group">
                    <span class="control-label">Hidden files:</span>
                    <a href="{}" class="btn {}">{}</a>
                </div>"#,
            hidden_toggle_link, hidden_btn_class, hidden_text
        );
    } else {
        let _ = write!(
            html,
            r#"
                <div class="control-group">
                    <span class="control-label">Hidden files:</span>
                    <span class="btn disabled" title="Disabled by server">Disabled</span>
                </div>"#
        );
    }

    let _ = write!(
        html,
        r#"
            </div>
        </header>
        <div class="file-list">
            <div class="file-header">
                <span></span>
                <span>Name</span>
                <span class="size">Size</span>
                <span class="time">Modified</span>
            </div>"#
    );

    for entry in entries {
        let size_display = if entry.is_dir {
            "-".to_string()
        } else {
            format_size(entry.size)
        };
        let time_display = entry
            .modified_time
            .map(format_http_date)
            .unwrap_or_else(|| "-".to_string());

        let is_dir_class = if entry.is_dir { "dir" } else { "" };

        let _ = write!(
            html,
            r#"
            <div class="file-item">
                <span class="icon">{}</span>
                <div class="filename">
                    <a href="{}" class="{}">
                        {}
                        {}
                    </a>
                </div>
                <span class="size">{}</span>
                <span class="time">{}</span>
            </div>"#,
            entry.file_type.icon(),
            entry.path,
            is_dir_class,
            html_escape(&entry.name),
            if entry.is_dir {
                String::new()
            } else {
                format!(
                    "<span class='file-type-badge' style='background: {}; color: #fff;'>{}</span>",
                    entry.file_type.color(),
                    format!("{:?}", entry.file_type)
                )
            },
            size_display,
            time_display
        );
    }

    let _ = write!(
        html,
        r#"
        </div>
        <footer>
            <p>D HTTP Server · {} items</p>
        </footer>
    </div>
</body>
</html>"#,
        entries.len()
    );

    html
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
