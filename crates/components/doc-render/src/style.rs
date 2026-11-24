//! Built-in light syntax highlight and document typography stylesheet rules.

/// Light palette lexical highlight and anchor rules for rendered documents and code.
pub const HIGHLIGHT_CSS: &str = r#"
.hl-kw { color: #cf222e; font-weight: 500; }
.hl-ty { color: #953800; }
.hl-fn { color: #8250df; }
.hl-str { color: #0a3069; }
.hl-num { color: #0550ae; }
.hl-cmt { color: #6e7781; font-style: italic; }
.hl-attr { color: #116329; }
.md-anchor {
  color: #656d76;
  margin-left: 6px;
  text-decoration: none;
  font-weight: 400;
  font-size: 0.85em;
  opacity: 0.65;
}
h1:hover .md-anchor,
h2:hover .md-anchor,
h3:hover .md-anchor,
h4:hover .md-anchor,
h5:hover .md-anchor,
h6:hover .md-anchor {
  opacity: 1;
  color: #0969da;
}
.md-task { list-style: none; }
.md-task input[type="checkbox"] { margin-right: 6px; vertical-align: middle; }
.md-footnotes {
  border-top: 1px solid #e5e7eb;
  margin-top: 28px;
  padding-top: 12px;
  font-size: 0.92em;
  color: #656d76;
}
"#;
