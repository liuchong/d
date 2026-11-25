//! Zero-dependency lightweight plain escaped implementation of [`Renderer`]
//! demonstrating swappable contract implementations across minimal assemblies.

use frame::{
    BoxFuture, ConfigValue, Context, Deps, FrameError, Plugin, PluginSchema,
    ValidationIssue,
};
use render_api::{
    HeadingEntry, MarkdownOptions, RenderService, RenderedDocument, Renderer,
};
use std::sync::Arc;

fn escape_html(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

/// Lightweight renderer outputting safe HTML-escaped monospace blocks.
#[derive(Debug, Default, Clone, Copy)]
pub struct PlainRenderer;

impl Renderer for PlainRenderer {
    fn render_markdown(
        &self,
        src: &str,
        _opts: &MarkdownOptions,
    ) -> RenderedDocument {
        let mut headings = Vec::new();
        for line in src.lines() {
            let trimmed = line.trim_start();
            if let Some(rest) = trimmed.strip_prefix("# ") {
                let text = rest.trim().to_string();
                if !text.is_empty() {
                    let id = text
                        .chars()
                        .map(|c| {
                            if c.is_ascii_alphanumeric() {
                                c.to_ascii_lowercase()
                            } else {
                                '-'
                            }
                        })
                        .collect();
                    headings.push(HeadingEntry { level: 1, text, id });
                }
            }
        }
        RenderedDocument {
            html: format!(
                "<pre class=\"d-plain-doc\">{}</pre>",
                escape_html(src)
            ),
            headings,
        }
    }

    fn highlight_code(&self, _lang: &str, code: &str) -> String {
        escape_html(code)
    }

    fn highlight_stylesheet(&self) -> &'static str {
        ".d-plain-doc { white-space: pre-wrap; font-family: ui-monospace, monospace; }"
    }
}

/// Plugin providing [`RenderService`] backed by [`PlainRenderer`].
#[derive(Debug, Default, Clone, Copy)]
pub struct PlainDocRenderPlugin;

impl Plugin for PlainDocRenderPlugin {
    fn name(&self) -> &str {
        "doc-render-plain"
    }

    fn deps(&self) -> Deps {
        Deps::empty()
    }

    fn schema(&self) -> PluginSchema {
        PluginSchema::new(
            "Zero-dependency plain HTML-escaped text alternative implementation of render-api",
        )
    }

    fn validate_config(
        &self,
        raw: &ConfigValue,
    ) -> Result<ConfigValue, FrameError> {
        match raw {
            ConfigValue::Null => Ok(ConfigValue::Null),
            ConfigValue::Map(m) if m.is_empty() => Ok(raw.clone()),
            ConfigValue::Map(m) => Err(FrameError::validation(
                m.keys()
                    .map(|k| ValidationIssue {
                        path: k.clone(),
                        message: format!(
                            "unknown doc-render-plain option `{k}`"
                        ),
                    })
                    .collect(),
            )),
            _ => Err(FrameError::validation(vec![ValidationIssue {
                path: "".to_string(),
                message: "doc-render-plain config must be Null or empty Map"
                    .to_string(),
            }])),
        }
    }

    fn apply(
        &self,
        ctx: Context,
        _config: ConfigValue,
    ) -> BoxFuture<'static, Result<(), FrameError>> {
        Box::pin(async move {
            let svc = Arc::new(RenderService::new(Arc::new(PlainRenderer)));
            ctx.provide(svc).await?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_renderer_escapes_html_and_rejects_bad_config() {
        let r = PlainRenderer;
        let doc = r.render_markdown(
            "# Hello\n<script>alert(1)</script>",
            &MarkdownOptions::default(),
        );
        assert!(doc.html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
        assert!(!doc.html.contains("<script>"));
        assert_eq!(doc.headings.len(), 1);
        assert_eq!(doc.headings[0].id, "hello");
        assert_eq!(r.highlight_code("rust", "a < b & c"), "a &lt; b &amp; c");

        let plugin = PlainDocRenderPlugin;
        assert!(plugin.validate_config(&ConfigValue::Null).is_ok());
        let err = plugin
            .validate_config(&ConfigValue::Bool(true))
            .unwrap_err();
        assert_eq!(err.kind, frame::ErrorKind::ValidationFailed);
    }
}
