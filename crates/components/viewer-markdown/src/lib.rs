//! Minimalist white Markdown viewer plugin using [`RenderService`].

use files_api::shell::{
    format_file_size, format_utc_datetime, html_escape, parent_dir_href,
    percent_encode_path, render_viewer_topbar, render_white_document,
};
use files_api::{
    FileCategory, FileViewer, ViewMode, ViewerInput, ViewerRegistryService,
};
use frame::{BoxFuture, ConfigValue, Context, Deps, FrameError, Plugin};
use http_api::{HttpResponse, StatusCode};
use render_api::{MarkdownOptions, RenderService};
use std::sync::Arc;

struct MarkdownViewer {
    render: Arc<RenderService>,
}

impl FileViewer for MarkdownViewer {
    fn name(&self) -> &str {
        "viewer-markdown"
    }

    fn priority(&self) -> i32 {
        20
    }

    fn supports(&self, category: FileCategory, mode: ViewMode) -> bool {
        category == FileCategory::Markdown
            && matches!(
                mode,
                ViewMode::Default | ViewMode::Preview | ViewMode::Viewer
            )
    }

    fn render(&self, input: ViewerInput<'_>) -> Option<HttpResponse> {
        let src = input.text?;
        let rendered = self
            .render
            .render_markdown(src, &MarkdownOptions::default());
        let css = self.render.highlight_stylesheet();

        if input.view_mode == ViewMode::Preview {
            let body = format!(
                "<article class=\"d-doc-body\">\n{}</article>",
                rendered.html
            );
            return Some(HttpResponse::html(
                StatusCode::OK,
                render_white_document(input.file_name, css, &body),
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
            "{topbar}\n<article class=\"d-doc-body\">\n{}</article>",
            rendered.html
        );
        Some(HttpResponse::html(
            StatusCode::OK,
            render_white_document(input.file_name, css, &body),
        ))
    }
}

/// Plugin registering [`MarkdownViewer`] into [`ViewerRegistryService`].
#[derive(Debug, Default, Clone, Copy)]
pub struct MarkdownViewerPlugin;

impl Plugin for MarkdownViewerPlugin {
    fn name(&self) -> &str {
        "viewer-markdown"
    }

    fn deps(&self) -> Deps {
        Deps::empty()
            .with::<ViewerRegistryService>()
            .with::<RenderService>()
    }

    fn apply(
        &self,
        ctx: Context,
        _config: ConfigValue,
    ) -> BoxFuture<'static, Result<(), FrameError>> {
        Box::pin(async move {
            let registry = ctx.require::<ViewerRegistryService>()?;
            let render = ctx.require::<RenderService>()?;
            let handle =
                registry.register(Arc::new(MarkdownViewer { render }))?;
            let _ = ctx.effect_sync("viewer-markdown-mount", move |scope| {
                scope.defer_async(move || handle.dispose());
                Ok(())
            })?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use render_api::{HeadingEntry, RenderedDocument, Renderer};
    use std::path::Path;

    struct StubRenderer;
    impl Renderer for StubRenderer {
        fn render_markdown(
            &self,
            src: &str,
            _opts: &MarkdownOptions,
        ) -> RenderedDocument {
            RenderedDocument {
                html: format!("<p>{}</p>", html_escape(src)),
                headings: vec![HeadingEntry {
                    level: 1,
                    text: "T".to_string(),
                    id: "t".to_string(),
                }],
            }
        }
        fn highlight_code(&self, _lang: &str, code: &str) -> String {
            html_escape(code)
        }
        fn highlight_stylesheet(&self) -> &'static str {
            ".hl-kw { color: #cf222e; }"
        }
    }

    #[test]
    fn renders_white_markdown_page_without_external_links() {
        let viewer = MarkdownViewer {
            render: Arc::new(RenderService::new(Arc::new(StubRenderer))),
        };
        let resp = viewer
            .render(ViewerInput {
                rel_path: "/docs/guide.md",
                file_name: "guide.md",
                full_path: Path::new("guide.md"),
                size: 12,
                modified: None,
                category: FileCategory::Markdown,
                view_mode: ViewMode::Default,
                mime: "text/markdown; charset=utf-8",
                allow_hidden: false,
                text: Some("hello"),
            })
            .expect("rendered");
        assert_eq!(resp.status, StatusCode::OK);
        if let http_api::BodyKind::Bytes(b) = resp.body {
            let html = String::from_utf8(b).unwrap();
            assert!(html.contains("#ffffff"));
            assert!(html.contains("/docs/guide.md?view=raw"));
            assert!(!html.contains("<script"));
        } else {
            panic!("expected bytes body");
        }
    }
}
