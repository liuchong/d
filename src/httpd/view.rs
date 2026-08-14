//! Text file viewers: HTML wrapper pages and content renderers.

use super::dir::FileType;
use super::file::{serve_file, serve_raw_file};
use super::html::html_escape;
use crate::utils::{format_http_date, format_size};
use axum::{
    http::HeaderMap,
    response::{Html, IntoResponse, Response},
};
use std::path::Path;
use tokio::fs;

pub(crate) async fn serve_preview(
    path: &Path,
    relative_path: &str,
    metadata: &std::fs::Metadata,
    file_type: &FileType,
) -> Response {
    let content = match fs::read_to_string(path).await {
        Ok(c) => c,
        Err(_) => {
            // Binary file, fallback to raw (metadata passed by the caller,
            // avoiding a second racy filesystem lookup).
            return serve_raw_file(path, metadata).await;
        }
    };

    let html = match file_type {
        FileType::Markdown => render_markdown(relative_path, &content),
        FileType::Org => render_org(relative_path, &content),
        FileType::Code => render_code(relative_path, &content, file_type),
        FileType::Text => render_text(relative_path, &content),
        _ => render_text(relative_path, &content),
    };

    Html(html).into_response()
}

pub(crate) async fn serve_file_viewer(
    relative_path: &str,
    full_path: &Path,
    metadata: &std::fs::Metadata,
    file_type: &FileType,
) -> Response {
    let content = match fs::read_to_string(full_path).await {
        Ok(c) => c,
        Err(_) => {
            // Binary file, serve directly
            return serve_file(full_path, metadata, &HeaderMap::new(), false)
                .await;
        }
    };

    let file_name = Path::new(relative_path)
        .file_name()
        .unwrap_or_default()
        .to_string_lossy();

    let size = format_size(metadata.len());
    let modified = metadata
        .modified()
        .ok()
        .map(format_http_date)
        .unwrap_or_default();

    let file_type_name = format!("{:?}", file_type);
    let file_type_icon = file_type.icon();
    let file_type_color = file_type.color();

    // Determine default preview content
    let preview_content = match file_type {
        FileType::Markdown => render_markdown_content(&content),
        FileType::Org => render_org_content(&content),
        FileType::Code => render_code_content(&content, full_path),
        FileType::Text => render_text_content(&content),
        _ => html_escape(&content),
    };

    let html = format!(
        r##"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>{}</title>
    <link rel="stylesheet" href="https://cdnjs.cloudflare.com/ajax/libs/highlight.js/11.9.0/styles/github-dark.min.css">
    <script src="https://cdnjs.cloudflare.com/ajax/libs/highlight.js/11.9.0/highlight.min.js"></script>
    <script src="https://cdnjs.cloudflare.com/ajax/libs/highlight.js/11.9.0/languages/rust.min.js"></script>
    <script src="https://cdnjs.cloudflare.com/ajax/libs/highlight.js/11.9.0/languages/javascript.min.js"></script>
    <script src="https://cdnjs.cloudflare.com/ajax/libs/highlight.js/11.9.0/languages/python.min.js"></script>
    <script src="https://cdnjs.cloudflare.com/ajax/libs/highlight.js/11.9.0/languages/go.min.js"></script>
    <script src="https://cdnjs.cloudflare.com/ajax/libs/highlight.js/11.9.0/languages/bash.min.js"></script>
    <script src="https://cdnjs.cloudflare.com/ajax/libs/highlight.js/11.9.0/languages/json.min.js"></script>
    <script src="https://cdnjs.cloudflare.com/ajax/libs/highlight.js/11.9.0/languages/yaml.min.js"></script>
    <script src="https://cdn.jsdelivr.net/npm/marked/marked.min.js"></script>
    <style>
        * {{ box-sizing: border-box; margin: 0; padding: 0; }}
        body {{
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif;
            line-height: 1.6;
            background: #0d1117;
            color: #c9d1d9;
            min-height: 100vh;
        }}
        .header {{
            background: #161b22;
            border-bottom: 1px solid #30363d;
            padding: 16px 24px;
            position: sticky;
            top: 0;
            z-index: 100;
        }}
        .file-info {{
            display: flex;
            align-items: center;
            gap: 12px;
            margin-bottom: 12px;
        }}
        .file-icon {{ font-size: 2rem; }}
        .file-details {{ flex: 1; }}
        .file-name {{
            font-size: 1.25rem;
            font-weight: 600;
            color: #f0f6fc;
            word-break: break-all;
        }}
        .file-meta {{
            font-size: 0.85rem;
            color: #8b949e;
            margin-top: 4px;
        }}
        .file-type-badge {{
            display: inline-flex;
            align-items: center;
            padding: 4px 10px;
            border-radius: 20px;
            font-size: 0.75rem;
            font-weight: 600;
            text-transform: uppercase;
            background: {};
            color: #fff;
        }}
        .actions {{
            display: flex;
            gap: 8px;
            flex-wrap: wrap;
        }}
        .btn {{
            display: inline-flex;
            align-items: center;
            gap: 6px;
            padding: 8px 16px;
            border-radius: 6px;
            font-size: 0.9rem;
            font-weight: 500;
            text-decoration: none;
            cursor: pointer;
            border: 1px solid #30363d;
            background: #21262d;
            color: #c9d1d9;
            transition: all 0.2s;
        }}
        .btn:hover {{
            background: #30363d;
            border-color: #8b949e;
        }}
        .btn.active {{
            background: #1f6feb;
            border-color: #1f6feb;
            color: #fff;
        }}
        .btn-secondary {{
            background: transparent;
        }}
        .content {{
            max-width: 1200px;
            margin: 0 auto;
            padding: 24px;
        }}
        .preview-container {{
            background: #161b22;
            border: 1px solid #30363d;
            border-radius: 12px;
            overflow: hidden;
        }}
        .preview-header {{
            background: #21262d;
            padding: 12px 16px;
            font-size: 0.85rem;
            color: #8b949e;
            border-bottom: 1px solid #30363d;
        }}
        .preview-content {{
            padding: 24px;
            overflow-x: auto;
        }}
        /* Markdown styles */
        .markdown-body {{
            line-height: 1.8;
        }}
        .markdown-body h1, .markdown-body h2, .markdown-body h3,
        .markdown-body h4, .markdown-body h5, .markdown-body h6 {{
            margin-top: 24px;
            margin-bottom: 16px;
            font-weight: 600;
            line-height: 1.25;
            color: #f0f6fc;
        }}
        .markdown-body h1 {{ font-size: 2em; border-bottom: 1px solid #30363d; padding-bottom: 10px; }}
        .markdown-body h2 {{ font-size: 1.5em; border-bottom: 1px solid #30363d; padding-bottom: 8px; }}
        .markdown-body p {{ margin-bottom: 16px; }}
        .markdown-body code {{
            background: #0d1117;
            padding: 2px 6px;
            border-radius: 4px;
            font-family: 'SF Mono', Monaco, monospace;
            font-size: 0.9em;
        }}
        .markdown-body pre {{
            background: #0d1117;
            padding: 16px;
            border-radius: 8px;
            overflow-x: auto;
            margin-bottom: 16px;
        }}
        .markdown-body pre code {{ background: none; padding: 0; }}
        .markdown-body ul, .markdown-body ol {{
            margin-bottom: 16px;
            padding-left: 2em;
        }}
        .markdown-body li {{ margin-bottom: 4px; }}
        .markdown-body a {{ color: #58a6ff; }}
        .markdown-body a:hover {{ text-decoration: underline; }}
        .markdown-body blockquote {{
            border-left: 4px solid #30363d;
            padding-left: 16px;
            margin-left: 0;
            color: #8b949e;
        }}
        .markdown-body table {{
            border-collapse: collapse;
            margin-bottom: 16px;
            width: 100%;
        }}
        .markdown-body th, .markdown-body td {{
            border: 1px solid #30363d;
            padding: 8px 12px;
            text-align: left;
        }}
        .markdown-body th {{ background: #0d1117; font-weight: 600; }}
        .markdown-body img {{ max-width: 100%; border-radius: 8px; }}
        /* Code styles */
        .code-block {{
            margin: 0;
            font-size: 0.9rem;
            line-height: 1.6;
        }}
        .code-block pre {{
            margin: 0;
            padding: 20px;
            overflow-x: auto;
        }}
        /* Org mode styles */
        .org-body {{
            font-family: 'SF Mono', Monaco, monospace;
            font-size: 0.9rem;
            line-height: 1.8;
            white-space: pre-wrap;
        }}
        .org-body .org-heading {{
            color: #7ee787;
            font-weight: 600;
        }}
        .org-body .org-todo {{ color: #ffa198; font-weight: 600; }}
        .org-body .org-done {{ color: #7ee787; }}
        .org-body .org-tag {{ color: #79c0ff; }}
        .org-body .org-source {{
            background: #0d1117;
            padding: 16px;
            border-radius: 8px;
            margin: 8px 0;
        }}
        .back-link {{
            display: inline-flex;
            align-items: center;
            gap: 6px;
            color: #58a6ff;
            text-decoration: none;
            margin-bottom: 16px;
            font-size: 0.9rem;
        }}
        .back-link:hover {{ text-decoration: underline; }}
    </style>
</head>
<body>
    <div class="header">
        <a href="./" class="back-link">← Back to directory</a>
        <div class="file-info">
            <span class="file-icon">{}</span>
            <div class="file-details">
                <div class="file-name">{}</div>
                <div class="file-meta">{} · {}</div>
            </div>
            <span class="file-type-badge">{}</span>
        </div>
        <div class="actions">
            <button class="btn active" onclick="showTab('preview')">📄 Preview</button>
            <button class="btn btn-secondary" onclick="showTab('raw')">📝 Raw</button>
            <a href="?view=download" class="btn btn-secondary">⬇️ Download</a>
        </div>
    </div>

    <div class="content">
        <div id="preview-tab" class="preview-container">
            <div class="preview-header">Preview</div>
            <div class="preview-content">
                {}
            </div>
        </div>

        <div id="raw-tab" class="preview-container" style="display: none;">
            <div class="preview-header">Raw</div>
            <div class="preview-content">
                <pre class="code-block"><code id="raw-content">{}</code></pre>
            </div>
        </div>
    </div>

    <script>
        function showTab(tab) {{
            document.getElementById('preview-tab').style.display = tab === 'preview' ? 'block' : 'none';
            document.getElementById('raw-tab').style.display = tab === 'raw' ? 'block' : 'none';
            document.querySelectorAll('.btn').forEach(b => b.classList.remove('active'));
            event.target.classList.add('active');
        }}

        // Highlight code blocks
        document.querySelectorAll('pre code').forEach((block) => {{
            hljs.highlightElement(block);
        }});
    </script>
</body>
</html>"##,
        html_escape(&file_name),
        file_type_color,
        file_type_icon,
        html_escape(&file_name),
        size,
        modified,
        file_type_name,
        preview_content,
        html_escape(&content)
    );

    Html(html).into_response()
}

fn render_markdown(_path: &str, content: &str) -> String {
    render_markdown_content(content)
}

fn render_markdown_content(content: &str) -> String {
    let escaped = html_escape(content);
    format!(
        r#"<div class="markdown-body" id="markdown-content">{}</div>
<script>
    document.getElementById('markdown-content').innerHTML = marked.parse(`{}`);
</script>"#,
        escaped.replace('`', "\\`"),
        escaped.replace('`', "\\`").replace('$', "\\$")
    )
}

fn render_org(_path: &str, content: &str) -> String {
    render_org_content(content)
}

fn render_org_content(content: &str) -> String {
    let mut html = String::new();
    html.push_str("<div class=\"org-body\">");

    for line in content.lines() {
        let processed = if line.starts_with("* ") {
            format!("<div class=\"org-heading\">{}</div>", html_escape(line))
        } else if line.starts_with("#+BEGIN_SRC")
            || line.starts_with("#+END_SRC")
        {
            format!("<div class=\"org-source\">{}</div>", html_escape(line))
        } else if line.starts_with("#+") {
            format!(
                "<div style=\"color: #8b949e;\">{}</div>",
                html_escape(line)
            )
        } else if line.contains("TODO") {
            line.replace("TODO", "<span class=\"org-todo\">TODO</span>")
                .replace("DONE", "<span class=\"org-done\">DONE</span>")
        } else {
            html_escape(line)
        };
        html.push_str(&processed);
        html.push('\n');
    }

    html.push_str("</div>");
    html
}

fn render_code(_path: &str, content: &str, _file_type: &FileType) -> String {
    render_code_content(content, &std::path::PathBuf::from(_path))
}

fn render_code_content(content: &str, path: &Path) -> String {
    let ext = path.extension().unwrap_or_default().to_string_lossy();
    let lang = get_language(&ext);

    format!(
        r#"<pre class="code-block"><code class="language-{}">{}</code></pre>
<script>hljs.highlightAll();</script>"#,
        lang,
        html_escape(content)
    )
}

fn render_text(_path: &str, content: &str) -> String {
    render_text_content(content)
}

fn render_text_content(content: &str) -> String {
    format!(
        "<pre class=\"code-block\"><code>{}</code></pre>",
        html_escape(content)
    )
}

fn get_language(ext: &str) -> &'static str {
    match ext.to_lowercase().as_str() {
        "rs" => "rust",
        "js" => "javascript",
        "ts" => "typescript",
        "jsx" => "jsx",
        "tsx" => "tsx",
        "py" => "python",
        "java" => "java",
        "c" => "c",
        "cpp" | "cc" | "cxx" | "hpp" => "cpp",
        "h" => "c",
        "go" => "go",
        "rb" => "ruby",
        "php" => "php",
        "swift" => "swift",
        "kt" => "kotlin",
        "scala" => "scala",
        "r" => "r",
        "sh" | "bash" | "zsh" => "bash",
        "ps1" => "powershell",
        "lua" => "lua",
        "elm" => "elm",
        "erl" | "hrl" => "erlang",
        "ex" | "exs" => "elixir",
        "fs" | "fsx" => "fsharp",
        "ml" | "mli" => "ocaml",
        "hs" | "lhs" => "haskell",
        "clj" | "cljs" => "clojure",
        "coffee" => "coffeescript",
        "cr" => "crystal",
        "dart" => "dart",
        "groovy" => "groovy",
        "nim" => "nim",
        "zig" => "zig",
        "v" => "v",
        "sql" => "sql",
        "json" => "json",
        "xml" => "xml",
        "yaml" | "yml" => "yaml",
        "toml" => "toml",
        "md" | "markdown" => "markdown",
        "html" | "htm" => "html",
        "css" => "css",
        _ => "plaintext",
    }
}
