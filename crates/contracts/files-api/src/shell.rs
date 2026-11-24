//! Minimalist white UI shell shared across directory listings and viewers.
//! Zero external links, zero dark backgrounds, zero emoji clutter.

use std::time::{SystemTime, UNIX_EPOCH};

/// Escape `&`, `<`, `>`, `"`, and `'` for safe HTML text and attribute emission.
pub fn html_escape(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#x27;"),
            _ => out.push(ch),
        }
    }
    out
}

/// Encode a single path segment using uppercase RFC 3986 percent-encoding.
pub fn percent_encode_segment(segment: &str) -> String {
    let mut out = String::with_capacity(segment.len());
    for &b in segment.as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push('%');
            out.push(hex_upper(b >> 4));
            out.push(hex_upper(b & 0x0f));
        }
    }
    out
}

/// Encode each slash-separated component of a relative URL path.
pub fn percent_encode_path(path: &str) -> String {
    path.split('/')
        .map(percent_encode_segment)
        .collect::<Vec<_>>()
        .join("/")
}

const fn hex_upper(nibble: u8) -> char {
    match nibble {
        0..=9 => (b'0' + nibble) as char,
        _ => (b'A' + (nibble - 10)) as char,
    }
}

/// Format byte size in concise human-readable form.
pub fn format_file_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    if bytes == 0 {
        return "0 B".to_string();
    }
    let mut size = bytes as f64;
    let mut idx = 0;
    while size >= 1024.0 && idx + 1 < UNITS.len() {
        size /= 1024.0;
        idx += 1;
    }
    if idx == 0 {
        format!("{bytes} B")
    } else {
        format!("{size:.1} {}", UNITS[idx])
    }
}

/// Format a [`SystemTime`] in UTC (`YYYY-MM-DD HH:MM`) using pure `std` arithmetic.
pub fn format_utc_datetime(time: Option<SystemTime>) -> String {
    let Some(t) = time else {
        return "-".to_string();
    };
    let secs = t.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let hour = rem / 3_600;
    let min = (rem % 3_600) / 60;
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{min:02}")
}

/// Convert Unix epoch days (1970-01-01 = 0) into Gregorian `(year, month, day)`.
pub const fn civil_from_days(days: i64) -> (i64, u8, u8) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    (year, m as u8, d as u8)
}

/// Shared minimalist white base CSS tokens and typography rules.
pub const WHITE_BASE_CSS: &str = r#"
:root {
  --bg: #ffffff;
  --bg-subtle: #f6f8fa;
  --border: #e5e7eb;
  --fg: #1f2328;
  --fg-muted: #656d76;
  --accent: #0969da;
}
* { box-sizing: border-box; }
html, body {
  margin: 0;
  padding: 0;
  background: #ffffff;
  color: #1f2328;
  font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", "PingFang SC", "Hiragino Sans GB", "Microsoft YaHei", sans-serif;
  font-size: 14px;
  line-height: 1.6;
}
a { color: #0969da; text-decoration: none; }
a:hover { text-decoration: underline; }
.d-shell {
  max-width: 980px;
  margin: 0 auto;
  padding: 20px 24px 48px;
  background: #ffffff;
}
.d-topbar {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  padding: 10px 14px;
  background: #f6f8fa;
  border: 1px solid #e5e7eb;
  border-radius: 6px;
  margin-bottom: 20px;
}
.d-topbar-left {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 10px;
  min-width: 0;
}
.d-title {
  font-weight: 600;
  color: #1f2328;
  word-break: break-all;
}
.d-meta {
  color: #656d76;
  font-size: 13px;
}
.d-actions {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 12px;
  font-size: 13px;
}
.d-actions a {
  padding: 2px 6px;
  border-radius: 4px;
}
.d-actions a.active {
  font-weight: 600;
  color: #1f2328;
  background: #ffffff;
  border: 1px solid #e5e7eb;
}
.d-doc-body {
  max-width: 820px;
  margin: 0 auto;
  line-height: 1.7;
  color: #1f2328;
  background: #ffffff;
}
.d-doc-body img, .d-media-frame img, .d-media-frame video {
  max-width: 100%;
  height: auto;
}
.d-doc-body pre, .d-code-wrap pre {
  background: #f6f8fa;
  border: 1px solid #e5e7eb;
  border-radius: 6px;
  padding: 14px 16px;
  overflow-x: auto;
  font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace;
  font-size: 13px;
  line-height: 1.5;
}
.d-doc-body code {
  font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace;
  font-size: 0.92em;
  background: #f6f8fa;
  padding: 1px 4px;
  border-radius: 3px;
}
.d-doc-body pre code {
  background: transparent;
  padding: 0;
}
.d-doc-body blockquote {
  margin: 12px 0;
  padding: 4px 14px;
  border-left: 3px solid #e5e7eb;
  color: #656d76;
}
.d-doc-body table {
  border-collapse: collapse;
  width: 100%;
  margin: 14px 0;
}
.d-doc-body th, .d-doc-body td {
  border: 1px solid #e5e7eb;
  padding: 6px 10px;
}
.d-doc-body th {
  background: #f6f8fa;
  font-weight: 600;
}
.d-media-frame {
  text-align: center;
  padding: 24px 0;
  background: #ffffff;
}
"#;

/// Render a complete self-contained white HTML document with optional extra CSS.
pub fn render_white_document(
    title: &str,
    extra_css: &str,
    body_html: &str,
) -> String {
    format!(
        "<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n<meta charset=\"UTF-8\">\n<meta name=\"viewport\" content=\"width=device-width, initial-scale=1.0\">\n<title>{title}</title>\n<style>\n{WHITE_BASE_CSS}\n{extra_css}\n</style>\n</head>\n<body>\n<main class=\"d-shell\">\n{body_html}\n</main>\n</body>\n</html>\n",
        title = html_escape(title),
    )
}

/// Render a single minimalist viewer toolbar (`<- Parent | File | meta | nav`).
pub fn render_viewer_topbar(
    parent_href: &str,
    file_name: &str,
    meta: &str,
    actions_html: &str,
) -> String {
    format!(
        "<header class=\"d-topbar\"><div class=\"d-topbar-left\"><a href=\"{parent}\">&larr; Back</a><span class=\"d-title\">{name}</span><span class=\"d-meta\">{meta}</span></div><nav class=\"d-actions\">{actions_html}</nav></header>",
        parent = html_escape(parent_href),
        name = html_escape(file_name),
        meta = html_escape(meta),
    )
}

/// Derive parent directory href from a URL path like `/dir/file.rs`.
pub fn parent_dir_href(rel_path: &str) -> String {
    let trimmed = rel_path.trim_start_matches('/');
    match trimmed.rsplit_once('/') {
        Some((parent, _)) if !parent.is_empty() => {
            format!("/{}/", percent_encode_path(parent))
        }
        _ => "/".to_string(),
    }
}
