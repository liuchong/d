//! Zero-dependency pure-Rust Markdown parser, embedded HTML passthrough, and
//! lexical code highlighter.

pub mod highlight;
pub mod inline;
pub mod markdown;
pub mod style;

use frame::{BoxFuture, ConfigValue, Context, Deps, FrameError, Plugin};
use render_api::{MarkdownOptions, RenderService, RenderedDocument, Renderer};
use std::sync::Arc;

/// Default pure-Rust renderer implementing [`Renderer`].
#[derive(Debug, Default, Clone, Copy)]
pub struct DefaultRenderer;

impl Renderer for DefaultRenderer {
    fn render_markdown(
        &self,
        src: &str,
        opts: &MarkdownOptions,
    ) -> RenderedDocument {
        markdown::render_markdown_document(src, opts)
    }

    fn highlight_code(&self, lang: &str, code: &str) -> String {
        highlight::highlight_code(lang, code)
    }

    fn highlight_stylesheet(&self) -> &'static str {
        style::HIGHLIGHT_CSS
    }
}

/// Plugin providing [`RenderService`].
#[derive(Debug, Default, Clone, Copy)]
pub struct DocRenderPlugin;

impl Plugin for DocRenderPlugin {
    fn name(&self) -> &str {
        "doc-render"
    }

    fn deps(&self) -> Deps {
        Deps::empty()
    }

    fn apply(
        &self,
        ctx: Context,
        _config: ConfigValue,
    ) -> BoxFuture<'static, Result<(), FrameError>> {
        Box::pin(async move {
            let svc = Arc::new(RenderService::new(Arc::new(DefaultRenderer)));
            ctx.provide(svc).await?;
            Ok(())
        })
    }
}
