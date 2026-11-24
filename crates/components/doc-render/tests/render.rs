//! BDD/TDD tests for pure-Rust `doc-render` Markdown, embedded HTML,
//! CJK/ASCII heading anchors, footnotes, GFM tables/tasks, and syntax highlight
//! (`T-M01`, `T-M02`, `T-M05`).

use doc_render::DefaultRenderer;
use render_api::{MarkdownOptions, Renderer};
use std::sync::Arc;

#[test]
fn markdown_commonmark_gfm_tables_tasks_footnotes_and_cjk_anchors() {
    let renderer = DefaultRenderer;
    let src = "\
# 架构 Overview

# 架构 Overview

| 左 | 中 | 右 |
| :--- | :---: | ---: |
| a | b | c |

- [x] 已完成
- [ ] 待办
  - 嵌套子项

> 引用段落

~~废弃~~ **加粗** *斜体* `let x = 1` <https://example.com> 文本[^1]

```rust
fn main() {
    let x = \"hi\"; // note
}
```

[^1]: 注释说明
";

    let rendered = renderer.render_markdown(src, &MarkdownOptions::default());
    let html = &rendered.html;

    // Duplicate CJK + ASCII heading anchors deduplicated.
    assert!(html.contains("<h1 id=\"架构-overview\">"), "got: {html}");
    assert!(html.contains("<h1 id=\"架构-overview-1\">"), "got: {html}");
    assert_eq!(rendered.headings.len(), 2);
    assert_eq!(rendered.headings[0].id, "架构-overview");
    assert_eq!(rendered.headings[1].id, "架构-overview-1");

    // GFM aligned table.
    assert!(html.contains("<table>"), "got: {html}");
    assert!(html.contains("style=\"text-align:center\""), "got: {html}");
    assert!(html.contains("style=\"text-align:right\""), "got: {html}");

    // Task list checkboxes.
    assert!(
        html.contains("<input type=\"checkbox\" checked disabled>"),
        "got: {html}"
    );
    assert!(
        html.contains("<input type=\"checkbox\" disabled>"),
        "got: {html}"
    );

    // Strikethrough, blockquote, inline code, autolink.
    assert!(html.contains("<del>废弃</del>"), "got: {html}");
    assert!(html.contains("<blockquote>"), "got: {html}");
    assert!(html.contains("<code>let x = 1</code>"), "got: {html}");
    assert!(
        html.contains("<a href=\"https://example.com\""),
        "got: {html}"
    );

    // Footnotes bidirectional anchors.
    assert!(html.contains("href=\"#fn-1\""), "got: {html}");
    assert!(html.contains("id=\"fn-1\""), "got: {html}");
    assert!(html.contains("href=\"#fnref-1\""), "got: {html}");

    // Fenced code highlighting.
    assert!(
        html.contains("<span class=\"hl-kw\">fn</span>"),
        "got: {html}"
    );
}

#[test]
fn markdown_native_embedded_html_passthrough_and_sanitizer() {
    let renderer = DefaultRenderer;
    let src = "\
<details>
<summary>展开说明</summary>

内部 **加粗** 文字与 `code`

</details>

<figure>
  <img src=\"a.png\" width=\"320\" alt=\"图1\">
  <figcaption>图1</figcaption>
</figure>

<table><tr><td rowspan=\"2\">合并</td><td>A</td></tr><tr><td>B</td></tr></table>

快捷键：<kbd>Ctrl</kbd>+<kbd>C</kbd>
";

    let rendered = renderer.render_markdown(src, &MarkdownOptions::default());
    let html = &rendered.html;

    assert!(html.contains("<details>"), "got: {html}");
    assert!(html.contains("<summary>展开说明</summary>"), "got: {html}");
    assert!(html.contains("<strong>加粗</strong>"), "got: {html}");
    assert!(html.contains("<figure>"), "got: {html}");
    assert!(html.contains("width=\"320\""), "got: {html}");
    assert!(html.contains("<td rowspan=\"2\">合并</td>"), "got: {html}");
    assert!(html.contains("<kbd>Ctrl</kbd>+<kbd>C</kbd>"), "got: {html}");
    assert!(
        !html.contains("&lt;details&gt;"),
        "must not escape block html"
    );
    assert!(!html.contains("&lt;kbd&gt;"), "must not escape inline html");

    let sanitized = renderer.render_markdown(
        src,
        &MarkdownOptions {
            allow_raw_html: true,
            heading_anchors: true,
            sanitizer: Some(Arc::new(|raw| {
                raw.replace("<details>", "<section>")
            })),
        },
    );
    assert!(sanitized.html.contains("<section>"));
    assert!(!sanitized.html.contains("<details>"));
}

#[test]
fn lexical_highlighter_covers_core_languages_and_light_css() {
    let renderer = DefaultRenderer;

    let rust_html = renderer.highlight_code(
        "rust",
        "fn main() {\n    let x = \"hi\"; // c\n    let n = 42;\n}",
    );
    assert!(
        rust_html.contains("<span class=\"hl-kw\">fn</span>"),
        "{rust_html}"
    );
    assert!(
        rust_html.contains("<span class=\"hl-str\">&quot;hi&quot;</span>"),
        "{rust_html}"
    );
    assert!(
        rust_html.contains("<span class=\"hl-cmt\">// c</span>"),
        "{rust_html}"
    );
    assert!(
        rust_html.contains("<span class=\"hl-num\">42</span>"),
        "{rust_html}"
    );

    for (lang, sample) in [
        ("python", "def greet(): # hi\n    return \"ok\""),
        ("javascript", "const port = 8080; // ok"),
        ("typescript", "interface Opts { port: number }"),
        ("go", "func main() { var x = 1 }"),
        ("bash", "# shell\necho \"hello\""),
        ("json", "{\"enabled\": true, \"port\": 8080}"),
        ("yaml", "host: localhost # comment"),
        ("toml", "[package]\nname = \"d\"\nport = 8080"),
        ("sql", "SELECT id FROM files WHERE size > 10;"),
        ("html", "<div class=\"box\"><!-- note --></div>"),
        ("css", ".box { color: #0969da; }"),
    ] {
        let out = renderer.highlight_code(lang, sample);
        assert!(
            out.contains("hl-"),
            "expected lexical highlight classes for {lang}, got: {out}"
        );
    }

    let css = renderer.highlight_stylesheet();
    assert!(css.contains(".hl-kw"));
    assert!(css.contains(".hl-str"));
    assert!(css.contains(".hl-cmt"));
    assert!(!css.contains("#0d1117"));
    assert!(!css.contains("#161b22"));
}
