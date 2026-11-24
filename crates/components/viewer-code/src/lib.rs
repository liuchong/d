//! Minimalist white server-highlighted source/text/org viewer plugin.

use files_api::shell::{
    format_file_size, format_utc_datetime, html_escape, parent_dir_href,
    percent_encode_path, render_viewer_topbar, render_white_document,
};
use files_api::{
    FileCategory, FileViewer, ViewMode, ViewerInput, ViewerRegistryService,
};
use frame::{BoxFuture, ConfigValue, Context, Deps, FrameError, Plugin};
use http_api::{HttpResponse, StatusCode};
use render_api::RenderService;
use std::path::Path;
use std::sync::Arc;

struct CodeViewer {
    render: Arc<RenderService>,
}

fn lang_for_path(path: &Path, category: FileCategory) -> &'static str {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    match ext.as_deref() {
        Some("rs") => "rust",
        Some("js" | "mjs" | "cjs" | "jsx") => "javascript",
        Some("ts" | "tsx") => "typescript",
        Some("py") => "python",
        Some("go") => "go",
        Some("sh" | "bash" | "zsh" | "fish") => "bash",
        Some("json") => "json",
        Some("yaml" | "yml") => "yaml",
        Some("toml") => "toml",
        Some("sql") => "sql",
        Some("html" | "htm" | "xml" | "svg") => "html",
        Some("css" | "scss") => "css",
        Some(
            "c" | "cpp" | "cc" | "cxx" | "h" | "hpp" | "java" | "zig" | "swift",
        ) => "cpp",
        _ => match category {
            FileCategory::Code => "rust",
            _ => "plain",
        },
    }
}

impl FileViewer for CodeViewer {
    fn name(&self) -> &str {
        "viewer-code"
    }

    fn priority(&self) -> i32 {
        10
    }

    fn supports(&self, category: FileCategory, mode: ViewMode) -> bool {
        matches!(
            category,
            FileCategory::Code | FileCategory::Text | FileCategory::Org
        ) && matches!(
            mode,
            ViewMode::Default | ViewMode::Preview | ViewMode::Viewer
        )
    }

    fn render(&self, input: ViewerInput<'_>) -> Option<HttpResponse> {
        let src = input.text?;
        let lang = lang_for_path(input.full_path, input.category);
        let highlighted = self.render.highlight_code(lang, src);
        let css = self.render.highlight_stylesheet();

        if input.view_mode == ViewMode::Preview {
            let body = format!(
                "<div class=\"d-code-wrap\"><pre><code>{highlighted}</code></pre></div>"
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
            "{} · {} · {}",
            format_file_size(input.size),
            lang,
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
            "{topbar}\n<section class=\"d-code-wrap\"><pre><code>{highlighted}</code></pre></section>"
        );
        Some(HttpResponse::html(
            StatusCode::OK,
            render_white_document(input.file_name, css, &body),
        ))
    }
}

/// Plugin registering [`CodeViewer`] into [`ViewerRegistryService`].
#[derive(Debug, Default, Clone, Copy)]
pub struct CodeViewerPlugin;

impl Plugin for CodeViewerPlugin {
    fn name(&self) -> &str {
        "viewer-code"
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
            let handle = registry.register(Arc::new(CodeViewer { render }))?;
            let _ = ctx.effect_sync("viewer-code-mount", move |scope| {
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
    use render_api::{MarkdownOptions, RenderedDocument, Renderer};

    struct StubHl;
    impl Renderer for StubHl {
        fn render_markdown(
            &self,
            _src: &str,
            _opts: &MarkdownOptions,
        ) -> RenderedDocument {
            unreachable!()
        }
        fn highlight_code(&self, lang: &str, code: &str) -> String {
            format!("<span class=\"hl-kw\" data-lang=\"{lang}\">{code}</span>")
        }
        fn highlight_stylesheet(&self) -> &'static str {
            ".hl-kw { color: #cf222e; }"
        }
    }

    #[test]
    fn renders_code_with_highlight_tokens_and_white_shell() {
        let viewer = CodeViewer {
            render: Arc::new(RenderService::new(Arc::new(StubHl))),
        };
        let resp = viewer
            .render(ViewerInput {
                rel_path: "/main.py",
                file_name: "main.py",
                full_path: Path::new("main.py"),
                size: 8,
                modified: None,
                category: FileCategory::Code,
                view_mode: ViewMode::Default,
                mime: "text/plain",
                allow_hidden: false,
                text: Some("def x(): pass"),
            })
            .expect("rendered");
        if let http_api::BodyKind::Bytes(b) = resp.body {
            let html = String::from_utf8(b).unwrap();
            assert!(html.contains("data-lang=\"python\""));
            assert!(html.contains(".hl-kw"));
            assert!(html.contains("#ffffff"));
        } else {
            panic!("expected bytes body");
        }
    }
}
