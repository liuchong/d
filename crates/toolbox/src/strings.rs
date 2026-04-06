//! String utilities

use std::borrow::Cow;

/// Truncate string to max length with ellipsis
pub fn truncate(s: &str, max_len: usize) -> Cow<'_, str> {
    if s.len() <= max_len {
        Cow::Borrowed(s)
    } else {
        Cow::Owned(format!("{}...", &s[..max_len.saturating_sub(3)]))
    }
}

/// Convert to kebab-case
pub fn to_kebab_case(s: &str) -> String {
    s.to_lowercase()
        .replace(" ", "-")
        .replace("_", "-")
}

/// Convert to snake_case
pub fn to_snake_case(s: &str) -> String {
    s.to_lowercase()
        .replace(" ", "_")
        .replace("-", "_")
}

/// Convert to camelCase
pub fn to_camel_case(s: &str) -> String {
    let parts: Vec<&str> = s.split(|c| c == ' ' || c == '_' || c == '-').collect();
    
    let mut result = String::new();
    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        if i == 0 {
            result.push_str(&part.to_lowercase());
        } else {
            let mut chars = part.chars();
            if let Some(first) = chars.next() {
                result.push(first.to_ascii_uppercase());
                result.extend(chars.flat_map(|c| c.to_lowercase()));
            }
        }
    }
    
    result
}

/// Convert to PascalCase
pub fn to_pascal_case(s: &str) -> String {
    let parts: Vec<&str> = s.split(|c| c == ' ' || c == '_' || c == '-').collect();
    
    let mut result = String::new();
    for part in parts {
        if part.is_empty() {
            continue;
        }
        let mut chars = part.chars();
        if let Some(first) = chars.next() {
            result.push(first.to_ascii_uppercase());
            result.extend(chars.flat_map(|c| c.to_lowercase()));
        }
    }
    
    result
}

/// Indent each line
pub fn indent(s: &str, prefix: &str) -> String {
    s.lines()
        .map(|line| format!("{}{}", prefix, line))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Wrap text at specified width
pub fn wrap_text(s: &str, width: usize) -> String {
    let mut result = String::new();
    let mut line_len = 0;
    
    for word in s.split_whitespace() {
        if line_len + word.len() + 1 > width && line_len > 0 {
            result.push('\n');
            line_len = 0;
        }
        if line_len > 0 {
            result.push(' ');
            line_len += 1;
        }
        result.push_str(word);
        line_len += word.len();
    }
    
    result
}

/// Remove ANSI escape sequences
pub fn strip_ansi(s: &str) -> String {
    // Simple ANSI regex: \x1b\[[0-9;]*m
    let mut result = String::new();
    let mut chars = s.chars().peekable();
    
    while let Some(c) = chars.next() {
        if c == '\x1b' && chars.peek() == Some(&'[') {
            chars.next(); // consume '['
            // Skip until 'm'
            while let Some(ch) = chars.next() {
                if ch == 'm' {
                    break;
                }
            }
        } else {
            result.push(c);
        }
    }
    
    result
}

/// Normalize whitespace (collapse multiple spaces)
pub fn normalize_whitespace(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Count lines (handles different line endings)
pub fn count_lines(s: &str) -> usize {
    s.lines().count()
}

/// Escape for shell usage
pub fn shell_escape(s: &str) -> String {
    if s.is_empty() {
        return "''".to_string();
    }
    
    if s.chars().all(|c| c.is_alphanumeric() || "_-./:,".contains(c)) {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\"'\"'"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("hello world this is long", 10), "hello w...");
    }

    #[test]
    fn test_kebab_case() {
        assert_eq!(to_kebab_case("Hello World"), "hello-world");
        assert_eq!(to_kebab_case("hello_world"), "hello-world");
    }

    #[test]
    fn test_snake_case() {
        assert_eq!(to_snake_case("Hello World"), "hello_world");
        assert_eq!(to_snake_case("hello-world"), "hello_world");
    }

    #[test]
    fn test_camel_case() {
        assert_eq!(to_camel_case("hello world"), "helloWorld");
        assert_eq!(to_camel_case("hello-world"), "helloWorld");
        assert_eq!(to_camel_case("hello_world"), "helloWorld");
    }

    #[test]
    fn test_pascal_case() {
        assert_eq!(to_pascal_case("hello world"), "HelloWorld");
        assert_eq!(to_pascal_case("hello-world"), "HelloWorld");
    }

    #[test]
    fn test_indent() {
        assert_eq!(indent("line1\nline2", "  "), "  line1\n  line2");
    }

    #[test]
    fn test_normalize_whitespace() {
        assert_eq!(normalize_whitespace("  hello   world  "), "hello world");
    }

    #[test]
    fn test_strip_ansi() {
        assert_eq!(strip_ansi("\x1b[31mred\x1b[0m"), "red");
    }
}
