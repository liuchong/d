//! Pure-Rust zero-dependency strict HTML whitelist sanitizer for untrusted Markdown mode.

const DROP_WITH_BODY_TAGS: &[&str] = &[
    "script", "style", "iframe", "object", "embed", "noscript", "svg", "math",
    "template", "textarea", "title",
];

const ALLOWED_TAGS: &[&str] = &[
    "p",
    "br",
    "hr",
    "div",
    "span",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "blockquote",
    "pre",
    "code",
    "em",
    "strong",
    "b",
    "i",
    "u",
    "s",
    "del",
    "ins",
    "sub",
    "sup",
    "mark",
    "kbd",
    "small",
    "ul",
    "ol",
    "li",
    "dl",
    "dt",
    "dd",
    "table",
    "thead",
    "tbody",
    "tfoot",
    "tr",
    "th",
    "td",
    "caption",
    "colgroup",
    "col",
    "a",
    "img",
    "figure",
    "figcaption",
    "details",
    "summary",
    "section",
    "article",
    "nav",
    "aside",
    "header",
    "footer",
    "main",
    "abbr",
    "cite",
    "q",
    "time",
    "audio",
    "video",
    "source",
];

const ALLOWED_ATTRS: &[&str] = &[
    "id", "class", "title", "alt", "href", "src", "width", "height", "align",
    "colspan", "rowspan", "scope", "open", "controls", "type", "datetime",
    "cite", "start", "reversed", "lang", "dir", "loading", "decoding",
    "checked", "disabled",
];

/// Strictly allow only safe HTML elements and attributes for untrusted Markdown mode.
///
/// Dangerous container tags (`<script>`, `<style>`, `<iframe>`, `<svg>`, `<math>`, etc.)
/// drop both tags and body (even when unclosed); disallowed tags are stripped; only
/// whitelisted attributes and safe URL schemes (`http://`, `https://`, `mailto:`, relative
/// paths) survive entity-decoding checks.
pub fn sanitize_html(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;

    while let Some(lt) = rest.find('<') {
        out.push_str(&rest[..lt]);
        let after_lt = &rest[lt + 1..];
        let Some(gt) = find_tag_end(after_lt) else {
            // Drop malformed/unclosed tag tails (including unclosed `<script`).
            return out;
        };
        let raw_tag = &after_lt[..gt];
        rest = &after_lt[gt + 1..];

        let trimmed = raw_tag.trim();
        if trimmed.is_empty()
            || trimmed.starts_with('!')
            || trimmed.starts_with('?')
        {
            continue;
        }
        let is_close = trimmed.starts_with('/');
        let name_slice = if is_close {
            trimmed[1..].trim_start()
        } else {
            trimmed
        };
        let tag_end = name_slice
            .find(|c: char| c.is_ascii_whitespace() || c == '/')
            .unwrap_or(name_slice.len());
        let tag_lower = name_slice[..tag_end].to_ascii_lowercase();
        if tag_lower.is_empty() {
            continue;
        }

        if DROP_WITH_BODY_TAGS.contains(&tag_lower.as_str()) {
            if !is_close {
                let close_needle = format!("</{tag_lower}");
                let rest_lower = rest.to_ascii_lowercase();
                if let Some(close_idx) = rest_lower.find(&close_needle) {
                    let after_close = &rest[close_idx + close_needle.len()..];
                    if let Some(end_gt) = find_tag_end(after_close) {
                        rest = &after_close[end_gt + 1..];
                        continue;
                    }
                }
                // Unclosed dangerous container tag: truncate remaining input cleanly.
                return out;
            }
            continue;
        }

        if !ALLOWED_TAGS.contains(&tag_lower.as_str()) {
            continue;
        }

        if is_close {
            out.push_str("</");
            out.push_str(&tag_lower);
            out.push('>');
            continue;
        }

        let self_close = trimmed.ends_with('/');
        let attrs_raw = if self_close {
            name_slice[tag_end..].trim_end_matches('/').trim()
        } else {
            name_slice[tag_end..].trim()
        };
        let clean_attrs = sanitize_attrs(attrs_raw);
        out.push('<');
        out.push_str(&tag_lower);
        if !clean_attrs.is_empty() {
            out.push(' ');
            out.push_str(&clean_attrs);
        }
        if self_close {
            out.push_str(" /");
        }
        out.push('>');
    }
    out.push_str(rest);
    out
}

fn find_tag_end(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut quote: Option<u8> = None;
    for (i, &b) in bytes.iter().enumerate() {
        match quote {
            Some(q) => {
                if b == q {
                    quote = None;
                }
            }
            None => match b {
                b'"' | b'\'' => quote = Some(b),
                b'>' => return Some(i),
                _ => {}
            },
        }
    }
    None
}

fn sanitize_attrs(raw: &str) -> String {
    let mut kept = Vec::new();
    let bytes = raw.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        let name_start = i;
        while i < bytes.len()
            && !bytes[i].is_ascii_whitespace()
            && bytes[i] != b'='
        {
            i += 1;
        }
        let attr_name = &raw[name_start..i];
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }

        let mut attr_val: Option<String> = None;
        if i < bytes.len() && bytes[i] == b'=' {
            i += 1;
            while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            if i < bytes.len() && (bytes[i] == b'"' || bytes[i] == b'\'') {
                let quote = bytes[i];
                i += 1;
                let vstart = i;
                while i < bytes.len() && bytes[i] != quote {
                    i += 1;
                }
                attr_val = Some(raw[vstart..i].to_string());
                if i < bytes.len() {
                    i += 1;
                }
            } else {
                let vstart = i;
                while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
                    i += 1;
                }
                attr_val = Some(raw[vstart..i].to_string());
            }
        }

        let lower_name = attr_name.to_ascii_lowercase();
        if lower_name == "style" {
            if let Some(ref v) = attr_val
                && let Some(safe_style) = sanitize_text_align_style(v)
            {
                kept.push(format!("style=\"{safe_style}\""));
            }
            continue;
        }
        if !ALLOWED_ATTRS.contains(&lower_name.as_str()) {
            continue;
        }
        if matches!(lower_name.as_str(), "href" | "src" | "cite") {
            let Some(ref v) = attr_val else {
                continue;
            };
            if !is_safe_uri(v) {
                continue;
            }
        }

        match attr_val {
            Some(v) => kept
                .push(format!("{lower_name}=\"{}\"", v.replace('"', "&quot;"))),
            None => kept.push(lower_name),
        }
    }
    kept.join(" ")
}

fn sanitize_text_align_style(raw: &str) -> Option<&'static str> {
    let compact: String = raw
        .chars()
        .filter(|c| !c.is_ascii_whitespace())
        .collect::<String>()
        .trim_end_matches(';')
        .to_ascii_lowercase();
    match compact.as_str() {
        "text-align:left" => Some("text-align:left"),
        "text-align:center" => Some("text-align:center"),
        "text-align:right" => Some("text-align:right"),
        _ => None,
    }
}

fn decode_html_entities(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let bytes = raw.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] == b'&' {
            let rest = &raw[i + 1..];
            if let Some(hex_rest) =
                rest.strip_prefix("#x").or_else(|| rest.strip_prefix("#X"))
            {
                let mut len = 0;
                while len < hex_rest.len()
                    && len < 6
                    && hex_rest.as_bytes()[len].is_ascii_hexdigit()
                {
                    len += 1;
                }
                if len > 0
                    && let Ok(code) = u32::from_str_radix(&hex_rest[..len], 16)
                    && let Some(ch) = char::from_u32(code)
                {
                    out.push(ch);
                    i += 3 + len;
                    if i < bytes.len() && bytes[i] == b';' {
                        i += 1;
                    }
                    continue;
                }
            } else if let Some(dec_rest) = rest.strip_prefix('#') {
                let mut len = 0;
                while len < dec_rest.len()
                    && len < 7
                    && dec_rest.as_bytes()[len].is_ascii_digit()
                {
                    len += 1;
                }
                if len > 0
                    && let Ok(code) = dec_rest[..len].parse::<u32>()
                    && let Some(ch) = char::from_u32(code)
                {
                    out.push(ch);
                    i += 2 + len;
                    if i < bytes.len() && bytes[i] == b';' {
                        i += 1;
                    }
                    continue;
                }
            } else if let Some(semi) =
                rest.find(';').filter(|&n| n > 0 && n <= 12)
            {
                let named = &rest[..semi];
                let decoded = match named.to_ascii_lowercase().as_str() {
                    "colon" => Some(':'),
                    "tab" => Some('\t'),
                    "newline" => Some('\n'),
                    "amp" => Some('&'),
                    "lt" => Some('<'),
                    "gt" => Some('>'),
                    "quot" => Some('"'),
                    "apos" => Some('\''),
                    _ => None,
                };
                if let Some(ch) = decoded {
                    out.push(ch);
                    i += 1 + semi + 1;
                    continue;
                }
            }
        }
        let ch = raw[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

fn is_safe_uri(raw: &str) -> bool {
    let decoded = decode_html_entities(raw);
    let cleaned: String = decoded
        .chars()
        .filter(|c| !c.is_ascii_whitespace() && !c.is_control())
        .collect::<String>()
        .to_ascii_lowercase();
    if cleaned.is_empty() {
        return false;
    }
    if cleaned.starts_with("//") {
        return false;
    }
    if cleaned.starts_with('/')
        || cleaned.starts_with("./")
        || cleaned.starts_with("../")
        || cleaned.starts_with('#')
        || cleaned.starts_with('?')
    {
        return true;
    }
    if cleaned.starts_with("http://")
        || cleaned.starts_with("https://")
        || cleaned.starts_with("mailto:")
    {
        return true;
    }
    // Relative file names/paths without an explicit scheme prefix before `/` or `?` or `#`.
    let head = cleaned.split(['/', '?', '#']).next().unwrap_or("");
    !head.contains(':')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_scripts_event_handlers_and_javascript_urls() {
        let dirty = "<p onclick=\"alert(1)\">hello</p><script>steal()</script><a href=\"javascript:evil()\">bad</a><a href=\"https://example.com\">good</a><img src=\" ./a.png \" onerror=x()>";
        let clean = sanitize_html(dirty);
        assert!(!clean.contains("script"));
        assert!(!clean.contains("steal()"));
        assert!(!clean.contains("onclick"));
        assert!(!clean.contains("onerror"));
        assert!(!clean.contains("javascript:"));
        assert!(clean.contains("<p>hello</p>"));
        assert!(clean.contains("<a href=\"https://example.com\">good</a>"));
        assert!(clean.contains("<img src=\" ./a.png \">"));
    }

    #[test]
    fn blocks_entity_encoded_schemes_quoted_gt_unclosed_tags_and_svg_math() {
        let entity_dec = "<a href=\"&#106;avascript:alert(1)\">x</a>";
        let entity_dec_no_semi =
            "<a href=\"&#0000106avascript:alert(1)\">x</a>";
        let entity_hex = "<a href=\"jav&#x61;script:alert(1)\">x</a>";
        let entity_colon = "<a href=\"javascript&colon;alert(1)\">x</a>";
        assert_eq!(sanitize_html(entity_dec), "<a>x</a>");
        assert_eq!(sanitize_html(entity_dec_no_semi), "<a>x</a>");
        assert_eq!(sanitize_html(entity_hex), "<a>x</a>");
        assert_eq!(sanitize_html(entity_colon), "<a>x</a>");

        let quoted_gt = "<a title=\">\" onclick=\"alert(1)\" href=\"https://ok.test\">safe</a>";
        let quoted_out = sanitize_html(quoted_gt);
        assert!(
            !quoted_out.contains("onclick"),
            "quoted `>` must not leak onclick: {quoted_out}"
        );
        assert!(quoted_out.contains("href=\"https://ok.test\""));

        let unclosed_script = "<p>ok</p><script>alert(1)";
        assert_eq!(sanitize_html(unclosed_script), "<p>ok</p>");

        let svg_xss = "<svg onload=\"alert(1)\"><a xlink:href=\"javascript:alert(1)\">click</a></svg><iframe src=\"x\">bad</iframe><link rel=\"stylesheet\" href=\"evil.css\"><form action=\"/p\"><input type=\"text\"></form>";
        let svg_out = sanitize_html(svg_xss);
        assert!(!svg_out.contains("svg"));
        assert!(!svg_out.contains("onload"));
        assert!(!svg_out.contains("xlink"));
        assert!(!svg_out.contains("iframe"));
        assert!(!svg_out.contains("bad"));
        assert!(!svg_out.contains("link"));
        assert!(!svg_out.contains("form"));
        assert!(!svg_out.contains("input"));

        // Safe GFM text-align style and arbitrary unsafe style filtering.
        assert_eq!(
            sanitize_html(
                "<th style=\"text-align:center\">h</th><p style=\"color:red\">x</p>"
            ),
            "<th style=\"text-align:center\">h</th><p>x</p>"
        );
    }
}
