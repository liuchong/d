//! Pure-Rust zero-dependency HTML whitelist sanitizer for untrusted Markdown mode.

const DROP_WITH_BODY_TAGS: &[&str] =
    &["script", "style", "iframe", "object", "embed", "noscript"];

const STRIP_SELF_TAGS: &[&str] = &["form", "input", "textarea", "base", "meta"];

/// Remove dangerous HTML elements (`<script>`, `<iframe>`, `<object>`, `<embed>`, `<style>`),
/// inline event handler attributes (`on*`), and unsafe URI schemes (`javascript:`, `vbscript:`,
/// `data:text/html`).
pub fn sanitize_html(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;

    while let Some(lt) = rest.find('<') {
        out.push_str(&rest[..lt]);
        let after_lt = &rest[lt + 1..];
        let Some(gt) = after_lt.find('>') else {
            out.push('<');
            out.push_str(after_lt);
            return out;
        };
        let raw_tag = &after_lt[..gt];
        rest = &after_lt[gt + 1..];

        let trimmed = raw_tag.trim();
        if trimmed.starts_with("!--") {
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

        if !is_close && DROP_WITH_BODY_TAGS.contains(&tag_lower.as_str()) {
            let close_needle = format!("</{tag_lower}");
            let rest_lower = rest.to_ascii_lowercase();
            if let Some(close_idx) = rest_lower.find(&close_needle) {
                let after_close = &rest[close_idx + close_needle.len()..];
                if let Some(end_gt) = after_close.find('>') {
                    rest = &after_close[end_gt + 1..];
                    continue;
                }
            }
            continue;
        }
        if DROP_WITH_BODY_TAGS.contains(&tag_lower.as_str())
            || STRIP_SELF_TAGS.contains(&tag_lower.as_str())
        {
            continue;
        }

        if is_close {
            out.push('<');
            out.push('/');
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
        if lower_name.starts_with("on") || lower_name == "srcdoc" {
            continue;
        }
        if matches!(
            lower_name.as_str(),
            "href" | "src" | "action" | "formaction"
        ) && let Some(ref v) = attr_val
            && is_unsafe_uri(v)
        {
            continue;
        }

        match attr_val {
            Some(v) => kept
                .push(format!("{lower_name}=\"{}\"", v.replace('"', "&quot;"))),
            None => kept.push(lower_name),
        }
    }
    kept.join(" ")
}

fn is_unsafe_uri(raw: &str) -> bool {
    let cleaned: String = raw
        .chars()
        .filter(|c| !c.is_ascii_whitespace() && !c.is_control())
        .collect::<String>()
        .to_ascii_lowercase();
    cleaned.starts_with("javascript:")
        || cleaned.starts_with("vbscript:")
        || cleaned.starts_with("data:text/html")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_scripts_event_handlers_and_javascript_urls() {
        let dirty = "<p onclick=\"alert(1)\">hello</p><script>steal()</script><a href=\"javascript:evil()\">bad</a><a href=\"https://example.com\">good</a><img src=\" ./a.png \" onerror=x()>";
        let clean = sanitize_html(dirty);
        assert!(!clean.contains("script"));
        assert!(!clean.contains("onclick"));
        assert!(!clean.contains("onerror"));
        assert!(!clean.contains("javascript:"));
        assert!(clean.contains("<p>hello</p>"));
        assert!(clean.contains("<a href=\"https://example.com\">good</a>"));
        assert!(clean.contains("<img src=\" ./a.png \">"));
    }
}
