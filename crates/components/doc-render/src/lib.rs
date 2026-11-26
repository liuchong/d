//! Zero-dependency pure-Rust Markdown parser, embedded HTML passthrough, and
//! lexical code highlighter.

pub mod highlight;
pub mod inline;
pub mod markdown;
pub mod sanitize;
pub mod style;

use frame::{
    BoxFuture, ConfigFieldSchema, ConfigFieldType, ConfigValue, Context, Deps,
    FrameError, Plugin, PluginSchema, ValidationIssue,
};
use render_api::{MarkdownOptions, RenderService, RenderedDocument, Renderer};
use std::sync::Arc;

/// Default pure-Rust renderer implementing [`Renderer`].
#[derive(Debug, Default, Clone, Copy)]
pub struct DefaultRenderer {
    pub sanitize_html: bool,
}

impl Renderer for DefaultRenderer {
    fn render_markdown(
        &self,
        src: &str,
        opts: &MarkdownOptions,
    ) -> RenderedDocument {
        let mut doc = markdown::render_markdown_document(src, opts);
        if self.sanitize_html || opts.sanitize_html {
            doc.html = sanitize::sanitize_html(&doc.html);
        }
        doc
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

fn parse_sanitize_html(raw: &ConfigValue) -> Result<bool, FrameError> {
    match raw {
        ConfigValue::Null => Ok(false),
        ConfigValue::Map(m) => {
            let mut sanitize_html = false;
            let mut issues = Vec::new();
            for (k, v) in m {
                match k.as_str() {
                    "sanitize_html" => match v {
                        ConfigValue::Bool(b) => sanitize_html = *b,
                        _ => issues.push(ValidationIssue {
                            path: k.clone(),
                            message: "sanitize_html must be a boolean"
                                .to_string(),
                        }),
                    },
                    _ => issues.push(ValidationIssue {
                        path: k.clone(),
                        message: format!("unknown doc-render option `{k}`"),
                    }),
                }
            }
            if issues.is_empty() {
                Ok(sanitize_html)
            } else {
                Err(FrameError::validation(issues))
            }
        }
        _ => Err(FrameError::validation(vec![ValidationIssue {
            path: "".to_string(),
            message: "doc-render config must be Null or Map".to_string(),
        }])),
    }
}

impl Plugin for DocRenderPlugin {
    fn name(&self) -> &str {
        "doc-render"
    }

    fn deps(&self) -> Deps {
        Deps::empty()
    }

    fn schema(&self) -> PluginSchema {
        PluginSchema::new(
            "Pure-Rust CommonMark+GFM renderer, embedded HTML passthrough, optional sanitization, and built-in lexical code highlighter",
        )
        .field(
            ConfigFieldSchema::new(
                "sanitize_html",
                ConfigFieldType::Bool,
                "Enable built-in whitelist HTML sanitizer for untrusted Markdown mode",
            )
            .with_default(ConfigValue::Bool(false)),
        )
    }

    fn validate_config(
        &self,
        raw: &ConfigValue,
    ) -> Result<ConfigValue, FrameError> {
        let _ = parse_sanitize_html(raw)?;
        Ok(raw.clone())
    }

    fn apply(
        &self,
        ctx: Context,
        config: ConfigValue,
    ) -> BoxFuture<'static, Result<(), FrameError>> {
        Box::pin(async move {
            let sanitize_html = parse_sanitize_html(&config)?;
            let svc = Arc::new(RenderService::new(Arc::new(DefaultRenderer {
                sanitize_html,
            })));
            ctx.provide(svc).await?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frame::{ErrorKind, FrameOptions, Spawner};
    use std::collections::BTreeMap;
    use std::future::Future;
    use std::pin::pin;
    use std::task::{Context as TaskContext, Poll, Waker};

    #[derive(Default)]
    struct InlineSpawner;

    impl Spawner for InlineSpawner {
        fn spawn(&self, fut: BoxFuture<'static, ()>) {
            block_on_ready(fut);
        }
    }

    fn block_on_ready<F: Future>(fut: F) -> F::Output {
        let mut fut = pin!(fut);
        let waker = Waker::noop();
        let mut cx = TaskContext::from_waker(waker);
        for _ in 0..1024 {
            if let Poll::Ready(out) = fut.as_mut().poll(&mut cx) {
                return out;
            }
            std::thread::yield_now();
        }
        panic!("future did not settle within 1024 polls");
    }

    #[test]
    fn doc_render_plugin_toggles_sanitize_html_and_validates() {
        assert_eq!(
            DocRenderPlugin
                .validate_config(&ConfigValue::Map(BTreeMap::from([(
                    "unknown".to_string(),
                    ConfigValue::Bool(true)
                )])))
                .unwrap_err()
                .kind,
            ErrorKind::ValidationFailed
        );

        let src = "# Hi\n\n<script>alert(1)</script><p onclick=\"x()\">ok</p>";
        let ctx_raw = Context::new(FrameOptions::new(Arc::new(InlineSpawner)));
        ctx_raw
            .plugin(DocRenderPlugin, ConfigValue::Null)
            .expect("raw apply");
        block_on_ready(ctx_raw.settle());
        let raw_html = ctx_raw
            .get::<RenderService>()
            .unwrap()
            .render_markdown(src, &MarkdownOptions::default())
            .html;
        assert!(raw_html.contains("<script>"));
        block_on_ready(ctx_raw.dispose()).unwrap();

        let ctx_safe = Context::new(FrameOptions::new(Arc::new(InlineSpawner)));
        ctx_safe
            .plugin(
                DocRenderPlugin,
                ConfigValue::Map(BTreeMap::from([(
                    "sanitize_html".to_string(),
                    ConfigValue::Bool(true),
                )])),
            )
            .expect("safe apply");
        block_on_ready(ctx_safe.settle());
        let safe_html = ctx_safe
            .get::<RenderService>()
            .unwrap()
            .render_markdown(src, &MarkdownOptions::default())
            .html;
        assert!(!safe_html.contains("<script"));
        assert!(!safe_html.contains("onclick"));
        assert!(safe_html.contains("<p>ok</p>"));
        block_on_ready(ctx_safe.dispose()).unwrap();
    }
}
