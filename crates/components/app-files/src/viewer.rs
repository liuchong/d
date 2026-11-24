//! Pluggable viewer registry and Stage 2 minimalist white fallback viewer.

use files_api::shell::{
    format_file_size, format_utc_datetime, html_escape, parent_dir_href,
    percent_encode_path, render_viewer_topbar, render_white_document,
};
use files_api::{
    FileCategory, FileViewer, ViewMode, ViewerInput, ViewerRegistry,
};
use frame::{Context, EffectHandle, FrameError};
use http_api::{HttpResponse, StatusCode};
use std::cmp::Reverse;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

struct Entry {
    seq: u64,
    priority: i32,
    viewer: Arc<dyn FileViewer>,
}

struct RegistryInner {
    ctx: Context,
    next_seq: AtomicU64,
    viewers: Mutex<Vec<Entry>>,
}

/// Default thread-safe [`ViewerRegistry`] managed by `frame` effects.
pub struct DefaultViewerRegistry {
    inner: Arc<RegistryInner>,
}

impl DefaultViewerRegistry {
    /// Construct a viewer registry anchored to `ctx`.
    pub fn new(ctx: Context) -> Arc<Self> {
        Arc::new(Self {
            inner: Arc::new(RegistryInner {
                ctx,
                next_seq: AtomicU64::new(1),
                viewers: Mutex::new(Vec::new()),
            }),
        })
    }
}

impl ViewerRegistry for DefaultViewerRegistry {
    fn register(
        &self,
        viewer: Arc<dyn FileViewer>,
    ) -> Result<EffectHandle, FrameError> {
        let seq = self.inner.next_seq.fetch_add(1, Ordering::Relaxed);
        let label = format!("file-viewer:{}", viewer.name());
        let priority = viewer.priority();
        let inner_add = Arc::clone(&self.inner);
        let inner_del = Arc::clone(&self.inner);
        self.inner.ctx.effect_sync(label, move |scope| {
            {
                let mut guard =
                    inner_add.viewers.lock().unwrap_or_else(|e| e.into_inner());
                guard.push(Entry {
                    seq,
                    priority,
                    viewer,
                });
                guard.sort_by_key(|e| (Reverse(e.priority), e.seq));
            }
            scope.defer(move || {
                let mut guard =
                    inner_del.viewers.lock().unwrap_or_else(|e| e.into_inner());
                guard.retain(|e| e.seq != seq);
            });
            Ok(())
        })
    }

    fn resolve_response(&self, input: ViewerInput<'_>) -> Option<HttpResponse> {
        let list: Vec<Arc<dyn FileViewer>> = {
            let guard =
                self.inner.viewers.lock().unwrap_or_else(|e| e.into_inner());
            guard.iter().map(|e| Arc::clone(&e.viewer)).collect()
        };
        for viewer in list {
            if viewer.supports(input.category, input.view_mode)
                && let Some(resp) = viewer.render(ViewerInput { ..input })
            {
                return Some(resp);
            }
        }
        None
    }
}

/// Baseline fallback text/code viewer active until specialized viewers register.
pub(crate) struct FallbackTextViewer;

impl FileViewer for FallbackTextViewer {
    fn name(&self) -> &str {
        "fallback-text"
    }

    fn priority(&self) -> i32 {
        -100
    }

    fn supports(&self, category: FileCategory, mode: ViewMode) -> bool {
        matches!(
            mode,
            ViewMode::Default | ViewMode::Preview | ViewMode::Viewer
        ) && matches!(
            category,
            FileCategory::Code
                | FileCategory::Text
                | FileCategory::Markdown
                | FileCategory::Org
        )
    }

    fn render(&self, input: ViewerInput<'_>) -> Option<HttpResponse> {
        let text = input.text?;
        let escaped = html_escape(text);
        if input.view_mode == ViewMode::Preview {
            let body = format!(
                "<div class=\"d-code-wrap\"><pre><code>{escaped}</code></pre></div>"
            );
            return Some(HttpResponse::html(
                StatusCode::OK,
                render_white_document(input.file_name, "", &body),
            ));
        }

        let encoded = format!(
            "/{}",
            percent_encode_path(input.rel_path.trim_start_matches('/'))
        );
        let parent = parent_dir_href(input.rel_path);
        let meta = format!(
            "{} · {}",
            format_file_size(input.size),
            format_utc_datetime(input.modified)
        );
        let actions = format!(
            "<a class=\"active\" href=\"{p}\">Preview</a>\
             <a href=\"{p}?view=raw\">Raw</a>\
             <a href=\"{p}?view=download\">Download</a>",
            p = html_escape(&encoded),
        );
        let topbar =
            render_viewer_topbar(&parent, input.file_name, &meta, &actions);
        let body = format!(
            "{topbar}\n<section class=\"d-code-wrap\"><pre><code>{escaped}</code></pre></section>"
        );
        Some(HttpResponse::html(
            StatusCode::OK,
            render_white_document(input.file_name, "", &body),
        ))
    }
}
