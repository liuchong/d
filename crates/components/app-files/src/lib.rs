//! Layer 4 file server plugin: sandboxed path resolution, static streaming,
//! trailing-slash 301 canonicalization, directory listings, and viewer registry.

pub mod dir;
pub mod file;
pub mod httpdate;
pub mod mime;
pub mod path;
pub mod percent;
pub mod viewer;

use dir::{serve_directory, try_serve_index};
use file::{serve_download, serve_file, serve_raw_file};
use files_api::{
    FileCategory, FileViewer, ViewMode, ViewerInput, ViewerRegistry,
    ViewerRegistryService,
};
use frame::{
    BoxFuture, ConfigValue, Context, Deps, FrameError, Plugin, ValidationIssue,
};
use http_api::{
    HttpHandler, HttpRequest, HttpResponse, Method, RequestMeta, RouteMatch,
    RouteSpec, RouterService, StatusCode,
};
pub use httpdate::{format_http_date, parse_http_date};
pub use mime::mime_for_path;
pub use path::{PathResolution, resolve_path, sanitize_path};
pub use percent::decode_path;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::fs;
use viewer::{DefaultViewerRegistry, FallbackTextViewer};

const MAX_VIEWER_TEXT_BYTES: u64 = 4 * 1024 * 1024;

/// Configuration for [`FilesAppPlugin`].
#[derive(Debug, Clone, Default)]
pub struct FilesConfig {
    pub root: PathBuf,
    pub allow_hidden: bool,
}

impl FilesConfig {
    /// Construct a file application config for canonical directory `root`.
    pub fn new(root: PathBuf, allow_hidden: bool) -> Self {
        Self { root, allow_hidden }
    }
}

/// Layer 4 plugin mounting the file serving router and [`ViewerRegistryService`].
#[derive(Debug, Clone, Default)]
pub struct FilesAppPlugin {
    config: FilesConfig,
}

impl FilesAppPlugin {
    /// Create a file application plugin for `config`.
    pub fn new(config: FilesConfig) -> Self {
        Self { config }
    }
}

fn resolve_files_config(
    base: &FilesConfig,
    raw: &ConfigValue,
) -> Result<FilesConfig, FrameError> {
    let mut cfg = base.clone();
    let mut issues = Vec::new();
    match raw {
        ConfigValue::Null => {}
        ConfigValue::Map(map) => {
            for (k, v) in map {
                match k.as_str() {
                    "root" => match v {
                        ConfigValue::Str(s) if !s.trim().is_empty() => {
                            cfg.root = PathBuf::from(s);
                        }
                        _ => issues.push(ValidationIssue {
                            path: k.clone(),
                            message: "root must be a non-empty path string"
                                .to_string(),
                        }),
                    },
                    "allow_hidden" => match v {
                        ConfigValue::Bool(b) => cfg.allow_hidden = *b,
                        _ => issues.push(ValidationIssue {
                            path: k.clone(),
                            message: "allow_hidden must be a boolean"
                                .to_string(),
                        }),
                    },
                    _ => issues.push(ValidationIssue {
                        path: k.clone(),
                        message: format!("unknown app-files option `{k}`"),
                    }),
                }
            }
        }
        _ => {
            return Err(FrameError::validation(vec![ValidationIssue {
                path: "".to_string(),
                message: "app-files config must be Null or Map".to_string(),
            }]));
        }
    }
    if cfg.root.as_os_str().is_empty() {
        issues.push(ValidationIssue {
            path: "root".to_string(),
            message: "root directory must be configured".to_string(),
        });
    }
    if issues.is_empty() {
        Ok(cfg)
    } else {
        Err(FrameError::validation(issues))
    }
}

impl Plugin for FilesAppPlugin {
    fn name(&self) -> &str {
        "app-files"
    }

    fn deps(&self) -> Deps {
        Deps::empty().with::<RouterService>()
    }

    fn validate_config(
        &self,
        raw: &ConfigValue,
    ) -> Result<ConfigValue, FrameError> {
        let _ = resolve_files_config(&self.config, raw)?;
        Ok(raw.clone())
    }

    fn apply(
        &self,
        ctx: Context,
        config: ConfigValue,
    ) -> BoxFuture<'static, Result<(), FrameError>> {
        let base = self.config.clone();
        Box::pin(async move {
            let config = resolve_files_config(&base, &config)?;
            let registry = DefaultViewerRegistry::new();
            let fallback: Arc<dyn FileViewer> = Arc::new(FallbackTextViewer);
            let _fallback_handle = registry.register(&ctx, fallback)?;
            let registry_svc = Arc::new(ViewerRegistryService::new(
                Arc::clone(&registry) as Arc<dyn ViewerRegistry>,
            ));
            ctx.provide(Arc::clone(&registry_svc)).await?;

            let router = ctx.use_service::<RouterService>()?;
            let handler: Arc<dyn HttpHandler> = Arc::new(FilesHandler {
                config,
                registry: registry_svc,
            });
            let _route_handle = router.mount_route(
                RouteSpec {
                    id: "app-files-catchall".to_string(),
                    host: None,
                    matcher: RouteMatch::CatchAll,
                    methods: Some(vec![Method::Get, Method::Head]),
                    priority: -100,
                },
                handler,
            )?;

            Ok(())
        })
    }
}

struct FilesHandler {
    config: FilesConfig,
    registry: Arc<ViewerRegistryService>,
}

impl HttpHandler for FilesHandler {
    fn call(&self, req: HttpRequest) -> BoxFuture<'static, HttpResponse> {
        let config = self.config.clone();
        let registry = Arc::clone(&self.registry);
        let meta = req.request_meta();
        Box::pin(async move { handle_request(&meta, &config, &registry).await })
    }
}

async fn handle_request(
    req: &RequestMeta,
    config: &FilesConfig,
    registry: &ViewerRegistryService,
) -> HttpResponse {
    let Ok(decoded) = decode_path(&req.path) else {
        return HttpResponse::text(StatusCode::BAD_REQUEST, "Invalid encoding");
    };

    if decoded == "/favicon.ico" {
        return HttpResponse::empty(StatusCode::NOT_FOUND);
    }

    let is_head = req.method == Method::Head;

    let full_path = match resolve_path(&config.root, &decoded).await {
        PathResolution::Resolved(p) => p,
        PathResolution::Invalid | PathResolution::NotFound => {
            return HttpResponse::text(StatusCode::NOT_FOUND, "Not Found");
        }
    };

    let metadata = match fs::metadata(&full_path).await {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return HttpResponse::text(StatusCode::NOT_FOUND, "Not Found");
        }
        Err(_) => {
            return HttpResponse::text(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Internal Server Error",
            );
        }
    };

    if metadata.is_dir() {
        if decoded != "/" && !decoded.ends_with('/') {
            let clean = req.path.trim_end_matches('/');
            let location = match &req.query {
                Some(q) if !q.is_empty() => format!("{clean}/?{q}"),
                _ => format!("{clean}/"),
            };
            return HttpResponse::redirect_moved(location);
        }

        if let Some(resp) =
            try_serve_index(&config.root, &full_path, req, is_head).await
        {
            return resp;
        }
        return serve_directory(
            &decoded,
            &full_path,
            req,
            config.allow_hidden,
            is_head,
        )
        .await;
    }

    let view_param = req.query_param("view");
    let Some(view_mode) = ViewMode::parse(view_param.as_deref()) else {
        return HttpResponse::text(StatusCode::BAD_REQUEST, "Invalid view");
    };

    match view_mode {
        ViewMode::Raw => serve_raw_file(&full_path, &metadata, is_head).await,
        ViewMode::Download => {
            serve_download(&full_path, &decoded, &metadata, is_head).await
        }
        ViewMode::Preview | ViewMode::Viewer => {
            let ext = file_ext(&full_path);
            let category = FileCategory::from_extension(&ext);
            let file_name = full_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(&decoded);
            let mime = mime_for_path(&full_path);

            let maybe_text =
                maybe_read_text_for_viewer(&full_path, &metadata, category)
                    .await;
            let input = ViewerInput {
                rel_path: &decoded,
                file_name,
                full_path: &full_path,
                size: metadata.len(),
                modified: metadata.modified().ok(),
                category,
                view_mode,
                mime,
                allow_hidden: config.allow_hidden,
                text: maybe_text.as_deref(),
            };
            if let Some(resp) = registry.resolve_response(input) {
                return apply_head_if_needed(resp, is_head);
            }
            serve_file(&full_path, &metadata, req, is_head).await
        }
        ViewMode::Default => {
            let ext = file_ext(&full_path);
            let category = FileCategory::from_extension(&ext);
            if category.is_document_or_code_page(&ext) {
                let maybe_text =
                    maybe_read_text_for_viewer(&full_path, &metadata, category)
                        .await;
                if maybe_text.is_some() {
                    let file_name = full_path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or(&decoded);
                    let mime = mime_for_path(&full_path);
                    let input = ViewerInput {
                        rel_path: &decoded,
                        file_name,
                        full_path: &full_path,
                        size: metadata.len(),
                        modified: metadata.modified().ok(),
                        category,
                        view_mode: ViewMode::Default,
                        mime,
                        allow_hidden: config.allow_hidden,
                        text: maybe_text.as_deref(),
                    };
                    if let Some(resp) = registry.resolve_response(input) {
                        return apply_head_if_needed(resp, is_head);
                    }
                }
            }
            serve_file(&full_path, &metadata, req, is_head).await
        }
    }
}

async fn maybe_read_text_for_viewer(
    full_path: &Path,
    metadata: &std::fs::Metadata,
    category: FileCategory,
) -> Option<String> {
    if !matches!(
        category,
        FileCategory::Code
            | FileCategory::Text
            | FileCategory::Markdown
            | FileCategory::Org
    ) || metadata.len() > MAX_VIEWER_TEXT_BYTES
    {
        return None;
    }
    let bytes = fs::read(full_path).await.ok()?;
    String::from_utf8(bytes).ok()
}

fn file_ext(path: &Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_string()
}

fn apply_head_if_needed(mut resp: HttpResponse, is_head: bool) -> HttpResponse {
    if is_head {
        resp.body = http_api::BodyKind::Empty;
    }
    resp
}
