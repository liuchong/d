//! HTTP server: router setup, graceful shutdown and request routing.

mod dir;
mod file;
mod html;
mod path;
mod view;

use crate::utils::decode_path;
use axum::{
    Router,
    body::Body,
    extract::{Query, Request, State},
    http::{HeaderMap, Method, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::get,
};
use serde::Deserialize;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use tokio::fs;
use tracing::{error, info, warn};

use self::dir::{FileType, serve_directory};
use self::file::{serve_download, serve_file, serve_raw_file};
use self::path::{PathResolution, resolve_path};
use self::view::{serve_file_viewer, serve_preview};

/// Server state
#[derive(Clone)]
pub struct ServerState {
    /// Canonicalized root directory. `main` canonicalizes the CLI root
    /// before calling [`start`], so all symlink-escape checks can compare
    /// against this path directly.
    pub(crate) root: PathBuf,
    pub(crate) allow_hidden: bool,
}

#[derive(Deserialize, Debug, Default, Clone, Copy)]
#[serde(rename_all = "lowercase")]
pub(crate) enum SortBy {
    #[default]
    Name,
    Size,
    Time,
    Type,
}

/// Unified query parameters shared by directory listings and file views.
#[derive(Deserialize, Clone)]
pub(crate) struct RequestQuery {
    #[serde(default)]
    sort: SortBy,
    hidden: Option<bool>,
    view: Option<String>,
    listing: Option<bool>,
}

/// Create the router.
///
/// `root` must already be canonicalized (see [`ServerState::root`]).
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

    if let Err(e) = serve.with_graceful_shutdown(shutdown_signal()).await {
        error!("Server error: {}", e);
    }

    info!("Server stopped");
}

/// Wait for a shutdown signal: SIGINT (Ctrl+C) on all platforms, plus
/// SIGTERM on Unix. Logs which signal triggered the shutdown.
#[cfg(unix)]
async fn shutdown_signal() {
    use tokio::signal::unix::{SignalKind, signal};

    let mut sigterm = match signal(SignalKind::terminate()) {
        Ok(s) => s,
        Err(e) => {
            error!("Failed to install SIGTERM handler: {}", e);
            // Fall back to listening for Ctrl+C only.
            match tokio::signal::ctrl_c().await {
                Ok(()) => info!("Received SIGINT (Ctrl+C), shutting down"),
                Err(e) => error!("Failed to listen for SIGINT: {}", e),
            }
            return;
        }
    };

    tokio::select! {
        res = tokio::signal::ctrl_c() => {
            match res {
                Ok(()) => info!("Received SIGINT (Ctrl+C), shutting down"),
                Err(e) => error!("Failed to listen for SIGINT: {}", e),
            }
        }
        _ = sigterm.recv() => {
            info!("Received SIGTERM, shutting down");
        }
    }
}

/// Wait for a shutdown signal: Ctrl+C only (non-Unix platforms).
#[cfg(not(unix))]
async fn shutdown_signal() {
    match tokio::signal::ctrl_c().await {
        Ok(()) => info!("Received shutdown signal (Ctrl+C)"),
        Err(e) => error!("Failed to listen for shutdown signal: {}", e),
    }
}

async fn handle_root(
    State(state): State<ServerState>,
    Query(query): Query<RequestQuery>,
    request: Request<Body>,
) -> impl IntoResponse {
    let headers = request.headers().clone();
    let is_head = request.method() == Method::HEAD;
    handle_dir("", &query, &state, &headers, is_head).await
}

async fn handle_request(
    State(state): State<ServerState>,
    Query(query): Query<RequestQuery>,
    request: Request<Body>,
) -> impl IntoResponse {
    let method = request.method().clone();
    let headers = request.headers().clone();
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

    let is_head = method == Method::HEAD;

    // Check if this is a view request for text files
    if let Some(view) = query.view.as_deref() {
        return handle_view(&decoded, view, &state).await;
    }

    handle_path(&decoded, &headers, is_head, &query, &state).await
}

async fn handle_view(path: &str, view: &str, state: &ServerState) -> Response {
    let full_path = match resolve_path(&state.root, path).await {
        PathResolution::Resolved(p) => p,
        PathResolution::Invalid => {
            warn!("Path traversal attempt blocked: {}", path);
            return (StatusCode::FORBIDDEN, "Invalid path").into_response();
        }
        PathResolution::NotFound => {
            return (StatusCode::NOT_FOUND, "Not Found").into_response();
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

    match view {
        "raw" => serve_raw_file(&full_path, &metadata).await,
        "preview" => {
            serve_preview(&full_path, path, &metadata, &file_type).await
        }
        "download" => serve_download(&full_path, path, &metadata).await,
        _ => (StatusCode::BAD_REQUEST, "Invalid view").into_response(),
    }
}

async fn handle_path(
    path: &str,
    req_headers: &HeaderMap,
    is_head: bool,
    query: &RequestQuery,
    state: &ServerState,
) -> Response {
    let full_path = match resolve_path(&state.root, path).await {
        PathResolution::Resolved(p) => p,
        PathResolution::Invalid => {
            warn!("Path traversal attempt blocked: {}", path);
            return (StatusCode::FORBIDDEN, "Invalid path").into_response();
        }
        PathResolution::NotFound => {
            return (StatusCode::NOT_FOUND, "Not Found").into_response();
        }
    };

    match fs::metadata(&full_path).await {
        Ok(metadata) if metadata.is_dir() => {
            if let Some(resp) =
                serve_index(state, &full_path, req_headers, is_head, query)
                    .await
            {
                return resp;
            }
            if is_head {
                (StatusCode::OK, Html("")).into_response()
            } else {
                serve_directory(path, &full_path, query, state).await
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
                serve_file(&full_path, &metadata, req_headers, is_head).await
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
    query: &RequestQuery,
    state: &ServerState,
    req_headers: &HeaderMap,
    is_head: bool,
) -> Response {
    let full_path = match resolve_path(&state.root, path).await {
        PathResolution::Resolved(p) => p,
        PathResolution::Invalid => {
            warn!("Path traversal attempt blocked: {}", path);
            return (StatusCode::FORBIDDEN, "Invalid path").into_response();
        }
        PathResolution::NotFound => {
            return (StatusCode::NOT_FOUND, "Not Found").into_response();
        }
    };

    match fs::metadata(&full_path).await {
        Ok(metadata) if metadata.is_dir() => {
            if let Some(resp) =
                serve_index(state, &full_path, req_headers, is_head, query)
                    .await
            {
                return resp;
            }
            serve_directory(path, &full_path, query, state).await
        }
        _ => (StatusCode::NOT_FOUND, "Not Found").into_response(),
    }
}

/// Serve a directory's `index.html` when one exists, unless the client
/// explicitly requested the listing with `?listing=true`. Returns `None`
/// when there is no usable index file. The index file is canonicalized and
/// checked against the server root, same as [`resolve_path`], so a symlink
/// pointing outside the root falls back to the plain listing.
async fn serve_index(
    state: &ServerState,
    dir: &Path,
    req_headers: &HeaderMap,
    is_head: bool,
    query: &RequestQuery,
) -> Option<Response> {
    if query.listing.unwrap_or(false) {
        return None;
    }

    let index = dir.join("index.html");
    let canonical = match fs::canonicalize(&index).await {
        Ok(p) if p.starts_with(&state.root) => p,
        Ok(p) => {
            warn!(
                "Symlink escape blocked: {} -> {}",
                index.display(),
                p.display()
            );
            return None;
        }
        Err(_) => return None,
    };

    match fs::metadata(&canonical).await {
        Ok(metadata) if metadata.is_file() => {
            Some(serve_file(&canonical, &metadata, req_headers, is_head).await)
        }
        _ => None,
    }
}
