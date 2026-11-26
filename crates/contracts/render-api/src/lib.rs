//! Document rendering and lexical code highlighting service contracts.

use frame::Service;
use std::fmt;
use std::sync::Arc;

/// Optional post-render HTML sanitization hook for untrusted content modes.
pub type HtmlSanitizer = Arc<dyn Fn(&str) -> String + Send + Sync>;

/// Rendering configuration passed to [`RenderService::render_markdown`].
#[derive(Clone)]
pub struct MarkdownOptions {
    /// Pass through embedded block and inline HTML elements unescaped when `true`.
    pub allow_raw_html: bool,
    /// Generate unique slug `id` attributes and anchor links for headings.
    pub heading_anchors: bool,
    /// Apply built-in whitelist HTML sanitization over rendered output when `true`.
    pub sanitize_html: bool,
    /// Optional sanitizer applied to rendered HTML before returning.
    pub sanitizer: Option<HtmlSanitizer>,
}

impl Default for MarkdownOptions {
    fn default() -> Self {
        Self {
            allow_raw_html: true,
            heading_anchors: true,
            sanitize_html: false,
            sanitizer: None,
        }
    }
}

impl fmt::Debug for MarkdownOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MarkdownOptions")
            .field("allow_raw_html", &self.allow_raw_html)
            .field("heading_anchors", &self.heading_anchors)
            .field("sanitize_html", &self.sanitize_html)
            .field("sanitizer", &self.sanitizer.as_ref().map(|_| ".."))
            .finish()
    }
}

/// Extracted heading entry from a rendered document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeadingEntry {
    pub level: u8,
    pub text: String,
    pub id: String,
}

/// Output of [`RenderService::render_markdown`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedDocument {
    pub html: String,
    pub headings: Vec<HeadingEntry>,
}

/// Pure-Rust server-side document rendering and highlighting contract trait.
pub trait Renderer: Send + Sync + 'static {
    /// Render Markdown source into semantic HTML and heading outline metadata.
    fn render_markdown(
        &self,
        src: &str,
        opts: &MarkdownOptions,
    ) -> RenderedDocument;

    /// Render syntax-highlighted HTML tokens for `code` in `lang`.
    fn highlight_code(&self, lang: &str, code: &str) -> String;

    /// Return built-in light stylesheet rules for lexical highlight token classes.
    fn highlight_stylesheet(&self) -> &'static str;
}

/// Frame service handle wrapping any [`Renderer`] implementation.
#[derive(Clone)]
pub struct RenderService(pub Arc<dyn Renderer>);

impl RenderService {
    /// Wrap a [`Renderer`] implementation as a registered service handle.
    pub fn new(inner: Arc<dyn Renderer>) -> Self {
        Self(inner)
    }

    /// Delegate [`Renderer::render_markdown`].
    pub fn render_markdown(
        &self,
        src: &str,
        opts: &MarkdownOptions,
    ) -> RenderedDocument {
        self.0.render_markdown(src, opts)
    }

    /// Delegate [`Renderer::highlight_code`].
    pub fn highlight_code(&self, lang: &str, code: &str) -> String {
        self.0.highlight_code(lang, code)
    }

    /// Delegate [`Renderer::highlight_stylesheet`].
    pub fn highlight_stylesheet(&self) -> &'static str {
        self.0.highlight_stylesheet()
    }
}

impl Service for RenderService {
    const NAME: &'static str = "render.service";
}
