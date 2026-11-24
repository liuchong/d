//! Inline Markdown parser: emphasis, links, images, autolinks, code spans,
//! strikethrough, footnotes, inline HTML passthrough, and CJK/ASCII heading slugs.

use crate::highlight::html_escape;
use std::collections::HashMap;
use std::fmt::Write as _;

/// Link reference target collected in document pre-pass.
#[derive(Debug, Clone)]
pub struct RefDef {
    pub url: String,
    pub title: Option<String>,
}

/// Shared document context threaded through inline rendering.
pub struct InlineCtx<'a> {
    pub allow_raw_html: bool,
    pub refs: &'a HashMap<String, RefDef>,
    pub footnotes: &'a HashMap<String, String>,
    pub fn_order: &'a mut Vec<String>,
    pub fn_indices: &'a mut HashMap<String, usize>,
}

/// Generate a stable, deduplicated heading `id` supporting CJK and ASCII tokens.
pub fn slugify_heading(
    text: &str,
    used_counts: &mut HashMap<String, usize>,
) -> String {
    let mut base = String::with_capacity(text.len());
    let mut pending_dash = false;
    for ch in text.chars() {
        if ch.is_alphanumeric() {
            if pending_dash && !base.is_empty() {
                base.push('-');
            }
            pending_dash = false;
            for lc in ch.to_lowercase() {
                base.push(lc);
            }
        } else if ch.is_whitespace() || ch == '-' || ch == '_' {
            pending_dash = true;
        }
    }
    if base.is_empty() {
        base.push_str("section");
    }

    let count = used_counts.entry(base.clone()).or_insert(0);
    let final_slug = if *count == 0 {
        base
    } else {
        format!("{base}-{}", *count)
    };
    *count += 1;
    final_slug
}

/// Strip inline Markdown punctuation to extract clean plain heading text.
pub fn heading_plain_text(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let bytes = raw.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if matches!(bytes[i], b'`' | b'*' | b'_' | b'~' | b'[') {
            i += 1;
        } else if bytes[i] == b']' && bytes.get(i + 1) == Some(&b'(') {
            if let Some(close) = raw[i + 2..].find(')') {
                i = i + 2 + close + 1;
            } else {
                out.push(']');
                i += 1;
            }
        } else if bytes[i] == b']' {
            i += 1;
        } else {
            let ch = raw[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    out.trim().to_string()
}

/// Render inline Markdown into HTML, preserving inline HTML when enabled.
pub fn render_inline(src: &str, ctx: &mut InlineCtx<'_>) -> String {
    render_inline_inner(src, ctx, false)
}

fn render_inline_inner(
    src: &str,
    ctx: &mut InlineCtx<'_>,
    inside_link: bool,
) -> String {
    let mut out = String::with_capacity(src.len() + 16);
    let bytes = src.as_bytes();
    let mut i = 0usize;

    while i < bytes.len() {
        let rest = &src[i..];

        // 1. Backslash escape or hard line break
        if bytes[i] == b'\\' && i + 1 < bytes.len() {
            let next = bytes[i + 1];
            if next == b'\n' {
                out.push_str("<br>\n");
                i += 2;
                continue;
            }
            if next.is_ascii_punctuation() {
                push_char_escaped(&mut out, next as char);
                i += 2;
                continue;
            }
        }

        // Trailing double-space hard break
        if rest.starts_with("  \n") {
            out.push_str("<br>\n");
            i += 3;
            continue;
        }

        // 2. Inline code span
        if bytes[i] == b'`' {
            let open_len = count_byte(bytes, i, b'`');
            if let Some(close_start) =
                find_closing_backticks(bytes, i + open_len, open_len)
            {
                let raw_inner = &src[i + open_len..close_start];
                let inner = strip_code_span_padding(raw_inner);
                out.push_str("<code>");
                out.push_str(&html_escape(inner));
                out.push_str("</code>");
                i = close_start + open_len;
                continue;
            }
            out.push_str(&src[i..i + open_len]);
            i += open_len;
            continue;
        }

        // 3. HTML comment
        if rest.starts_with("<!--")
            && let Some(end_rel) = rest[4..].find("-->")
        {
            let end = i + 4 + end_rel + 3;
            if ctx.allow_raw_html {
                out.push_str(&src[i..end]);
            } else {
                out.push_str(&html_escape(&src[i..end]));
            }
            i = end;
            continue;
        }

        // 4. Autolink <https://...> / <http://...> / <mailto:...>
        if bytes[i] == b'<'
            && let Some((url, end)) = parse_angle_autolink(src, i)
        {
            let href = html_escape(url);
            let label = html_escape(url.strip_prefix("mailto:").unwrap_or(url));
            let _ = write!(out, "<a href=\"{href}\">{label}</a>");
            i = end;
            continue;
        }

        // 5. Embedded inline HTML tag
        if bytes[i] == b'<'
            && let Some(end) = parse_html_tag_end(src, i)
        {
            if ctx.allow_raw_html {
                out.push_str(&src[i..end]);
            } else {
                out.push_str(&html_escape(&src[i..end]));
            }
            i = end;
            continue;
        }

        // 6. Footnote reference [^id]
        if rest.starts_with("[^")
            && let Some((id, end)) = parse_footnote_ref(src, i)
            && ctx.footnotes.contains_key(id)
        {
            let idx = if let Some(&existing) = ctx.fn_indices.get(id) {
                existing
            } else {
                ctx.fn_order.push(id.to_owned());
                let idx = ctx.fn_order.len();
                ctx.fn_indices.insert(id.to_owned(), idx);
                idx
            };
            let _ = write!(
                out,
                "<sup id=\"fnref-{idx}\"><a href=\"#fn-{idx}\">[{idx}]</a></sup>"
            );
            i = end;
            continue;
        }

        // 7. Image ![alt](dest) or ![alt][ref]
        if rest.starts_with("![")
            && let Some((alt, dest, title, end)) =
                parse_link_or_image(src, i + 1, ctx.refs)
        {
            let href = html_escape(&dest);
            let alt_esc = html_escape(&alt);
            if let Some(t) = title {
                let _ = write!(
                    out,
                    "<img src=\"{href}\" alt=\"{alt_esc}\" title=\"{}\">",
                    html_escape(&t)
                );
            } else {
                let _ = write!(out, "<img src=\"{href}\" alt=\"{alt_esc}\">");
            }
            i = end;
            continue;
        }

        // 8. Link [label](dest) or [label][ref]
        if !inside_link
            && bytes[i] == b'['
            && let Some((label, dest, title, end)) =
                parse_link_or_image(src, i, ctx.refs)
        {
            let label_html = render_inline_inner(&label, ctx, true);
            let href = html_escape(&dest);
            if let Some(t) = title {
                let _ = write!(
                    out,
                    "<a href=\"{href}\" title=\"{}\">{label_html}</a>",
                    html_escape(&t)
                );
            } else {
                let _ = write!(out, "<a href=\"{href}\">{label_html}</a>");
            }
            i = end;
            continue;
        }

        // 9. Strikethrough ~~...~~
        if rest.starts_with("~~")
            && let Some((inner, end)) = find_delimited(src, i + 2, "~~")
        {
            let body = render_inline_inner(inner, ctx, inside_link);
            let _ = write!(out, "<del>{body}</del>");
            i = end;
            continue;
        }

        // 10. Strong ** or __
        if (rest.starts_with("**") || rest.starts_with("__"))
            && can_open_delim(src, i)
        {
            let marker = &src[i..i + 2];
            if let Some((inner, end)) = find_delimited(src, i + 2, marker) {
                let body = render_inline_inner(inner, ctx, inside_link);
                let _ = write!(out, "<strong>{body}</strong>");
                i = end;
                continue;
            }
        }

        // 11. Emphasis * or _ (avoid intra-word foo_bar_baz for `_`)
        if (bytes[i] == b'*'
            || (bytes[i] == b'_' && is_word_left_boundary(src, i)))
            && can_open_delim(src, i)
        {
            let marker = &src[i..i + 1];
            if let Some((inner, end)) =
                find_single_emphasis(src, i + 1, bytes[i])
            {
                let _ = marker;
                let body = render_inline_inner(inner, ctx, inside_link);
                let _ = write!(out, "<em>{body}</em>");
                i = end;
                continue;
            }
        }

        // 12. Bare URL autolink
        if !inside_link
            && (rest.starts_with("https://") || rest.starts_with("http://"))
            && (i == 0
                || bytes[i - 1].is_ascii_whitespace()
                || matches!(bytes[i - 1], b'(' | b'['))
        {
            let end = scan_bare_url_end(src, i);
            if end > i + "http://x".len() {
                let url = &src[i..end];
                let esc = html_escape(url);
                let _ = write!(out, "<a href=\"{esc}\">{esc}</a>");
                i = end;
                continue;
            }
        }

        let ch = rest.chars().next().unwrap();
        push_char_escaped(&mut out, ch);
        i += ch.len_utf8();
    }

    out
}

fn push_char_escaped(out: &mut String, ch: char) {
    match ch {
        '&' => out.push_str("&amp;"),
        '<' => out.push_str("&lt;"),
        '>' => out.push_str("&gt;"),
        '"' => out.push_str("&quot;"),
        '\'' => out.push_str("&#x27;"),
        _ => out.push(ch),
    }
}

fn count_byte(bytes: &[u8], start: usize, needle: u8) -> usize {
    let mut n = 0;
    while start + n < bytes.len() && bytes[start + n] == needle {
        n += 1;
    }
    n
}

fn find_closing_backticks(
    bytes: &[u8],
    from: usize,
    open_len: usize,
) -> Option<usize> {
    let mut i = from;
    while i < bytes.len() {
        if bytes[i] == b'`' {
            let run = count_byte(bytes, i, b'`');
            if run == open_len {
                return Some(i);
            }
            i += run;
        } else {
            i += 1;
        }
    }
    None
}

fn strip_code_span_padding(raw: &str) -> &str {
    if raw.len() >= 2
        && raw.starts_with(' ')
        && raw.ends_with(' ')
        && raw.bytes().any(|b| b != b' ')
    {
        &raw[1..raw.len() - 1]
    } else {
        raw
    }
}

fn parse_angle_autolink(src: &str, start: usize) -> Option<(&str, usize)> {
    let rest = &src[start + 1..];
    let close = rest.find('>')?;
    let inner = &rest[..close];
    if inner.contains(char::is_whitespace) || inner.contains('<') {
        return None;
    }
    if inner.starts_with("https://")
        || inner.starts_with("http://")
        || inner.starts_with("mailto:")
    {
        Some((inner, start + 1 + close + 1))
    } else {
        None
    }
}

fn parse_html_tag_end(src: &str, start: usize) -> Option<usize> {
    let bytes = src.as_bytes();
    let mut i = start + 1;
    if i >= bytes.len() {
        return None;
    }
    if bytes[i] == b'/' {
        i += 1;
    }
    if i >= bytes.len() || !bytes[i].is_ascii_alphabetic() {
        return None;
    }
    while i < bytes.len()
        && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'-')
    {
        i += 1;
    }
    if i < bytes.len()
        && !bytes[i].is_ascii_whitespace()
        && bytes[i] != b'>'
        && bytes[i] != b'/'
    {
        return None;
    }
    let mut quote: Option<u8> = None;
    while i < bytes.len() {
        let b = bytes[i];
        if let Some(q) = quote {
            if b == q {
                quote = None;
            }
        } else if b == b'"' || b == b'\'' {
            quote = Some(b);
        } else if b == b'>' {
            return Some(i + 1);
        } else if b == b'<' {
            return None;
        }
        i += 1;
    }
    None
}

fn parse_footnote_ref(src: &str, start: usize) -> Option<(&str, usize)> {
    let close = src[start + 2..].find(']')?;
    let id = &src[start + 2..start + 2 + close];
    if id.is_empty()
        || id.contains(char::is_whitespace)
        || src.as_bytes().get(start + 2 + close + 1) == Some(&b':')
    {
        return None;
    }
    Some((id, start + 2 + close + 1))
}

fn parse_link_or_image(
    src: &str,
    bracket_start: usize,
    refs: &HashMap<String, RefDef>,
) -> Option<(String, String, Option<String>, usize)> {
    let label_end = find_matching_bracket(src, bracket_start)?;
    let label = &src[bracket_start + 1..label_end];
    let after = &src[label_end + 1..];

    if after.starts_with('(') {
        let (url, title, paren_end) = parse_inline_dest(after)?;
        return Some((label.to_owned(), url, title, label_end + 1 + paren_end));
    }

    if after.starts_with('[')
        && let Some(ref_close) = after[1..].find(']')
    {
        let raw_ref = &after[1..1 + ref_close];
        let key = if raw_ref.is_empty() { label } else { raw_ref };
        if let Some(def) = refs.get(&key.trim().to_ascii_lowercase()) {
            return Some((
                label.to_owned(),
                def.url.clone(),
                def.title.clone(),
                label_end + 1 + 1 + ref_close + 1,
            ));
        }
    }

    // Shortcut reference [label]
    if let Some(def) = refs.get(&label.trim().to_ascii_lowercase()) {
        return Some((
            label.to_owned(),
            def.url.clone(),
            def.title.clone(),
            label_end + 1,
        ));
    }

    None
}

fn find_matching_bracket(src: &str, start: usize) -> Option<usize> {
    let bytes = src.as_bytes();
    let mut depth = 0usize;
    let mut i = start;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() {
            i += 2;
            continue;
        }
        if bytes[i] == b'`' {
            let run = count_byte(bytes, i, b'`');
            if let Some(close) = find_closing_backticks(bytes, i + run, run) {
                i = close + run;
                continue;
            }
        }
        match bytes[i] {
            b'[' => depth += 1,
            b']' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

fn parse_inline_dest(after: &str) -> Option<(String, Option<String>, usize)> {
    // `after` starts with '('
    let bytes = after.as_bytes();
    let mut i = 1;
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    if i >= bytes.len() {
        return None;
    }

    let url = if bytes[i] == b'<' {
        let close = after[i + 1..].find('>')?;
        let u = &after[i + 1..i + 1 + close];
        i = i + 1 + close + 1;
        u.to_owned()
    } else {
        let start = i;
        let mut parens = 0usize;
        while i < bytes.len() {
            let b = bytes[i];
            if b == b'\\' && i + 1 < bytes.len() {
                i += 2;
                continue;
            }
            if b.is_ascii_whitespace() {
                break;
            }
            if b == b'(' {
                parens += 1;
            } else if b == b')' {
                if parens == 0 {
                    break;
                }
                parens -= 1;
            }
            i += 1;
        }
        after[start..i].to_owned()
    };

    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }

    let mut title = None;
    if i < bytes.len() && (bytes[i] == b'"' || bytes[i] == b'\'') {
        let q = bytes[i];
        let start = i + 1;
        i += 1;
        while i < bytes.len() && bytes[i] != q {
            if bytes[i] == b'\\' && i + 1 < bytes.len() {
                i += 2;
            } else {
                i += 1;
            }
        }
        if i >= bytes.len() {
            return None;
        }
        title = Some(after[start..i].to_owned());
        i += 1;
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
    }

    if i < bytes.len() && bytes[i] == b')' {
        Some((url, title, i + 1))
    } else {
        None
    }
}

fn can_open_delim(src: &str, idx: usize) -> bool {
    let run_char = src.as_bytes()[idx];
    let after = &src[idx..];
    let skipped = after
        .bytes()
        .position(|b| b != run_char)
        .unwrap_or(after.len());
    let next = src[idx + skipped..].chars().next();
    next.is_some_and(|c| !c.is_whitespace())
}

fn is_word_left_boundary(src: &str, idx: usize) -> bool {
    src[..idx]
        .chars()
        .next_back()
        .is_none_or(|c| !c.is_alphanumeric())
}

fn is_word_right_boundary(src: &str, idx_after: usize) -> bool {
    src[idx_after..]
        .chars()
        .next()
        .is_none_or(|c| !c.is_alphanumeric())
}

fn find_delimited<'a>(
    src: &'a str,
    from: usize,
    marker: &str,
) -> Option<(&'a str, usize)> {
    let bytes = src.as_bytes();
    let mut i = from;
    while i + marker.len() <= bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() {
            let escaped = src[i + 1..].chars().next().map_or(1, char::len_utf8);
            i += 1 + escaped;
            continue;
        }
        if bytes[i] == b'`' {
            let run = count_byte(bytes, i, b'`');
            if let Some(close) = find_closing_backticks(bytes, i + run, run) {
                i = close + run;
                continue;
            }
        }
        if src[i..].starts_with(marker) && i > from {
            let prev = src[..i].chars().next_back();
            if prev.is_some_and(|c| !c.is_whitespace()) {
                return Some((&src[from..i], i + marker.len()));
            }
        }
        i += src[i..].chars().next().map_or(1, char::len_utf8);
    }
    None
}

fn find_single_emphasis(
    src: &str,
    from: usize,
    ch: u8,
) -> Option<(&str, usize)> {
    let bytes = src.as_bytes();
    let mut i = from;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() {
            let escaped = src[i + 1..].chars().next().map_or(1, char::len_utf8);
            i += 1 + escaped;
            continue;
        }
        if bytes[i] == b'`' {
            let run = count_byte(bytes, i, b'`');
            if let Some(close) = find_closing_backticks(bytes, i + run, run) {
                i = close + run;
                continue;
            }
        }
        if bytes[i] == ch {
            if bytes.get(i + 1) == Some(&ch) {
                i += 2;
                continue;
            }
            if i > from
                && src[..i]
                    .chars()
                    .next_back()
                    .is_some_and(|c| !c.is_whitespace())
                && (ch != b'_' || is_word_right_boundary(src, i + 1))
            {
                return Some((&src[from..i], i + 1));
            }
        }
        i += src[i..].chars().next().map_or(1, char::len_utf8);
    }
    None
}

fn scan_bare_url_end(src: &str, start: usize) -> usize {
    let bytes = src.as_bytes();
    let mut i = start;
    while i < bytes.len() && !bytes[i].is_ascii_whitespace() && bytes[i] != b'<'
    {
        i += 1;
    }
    while i > start && matches!(bytes[i - 1], b'.' | b',' | b';' | b')' | b']')
    {
        i -= 1;
    }
    i
}
