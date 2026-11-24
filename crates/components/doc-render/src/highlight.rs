//! Pure-Rust lexical syntax highlighter covering system, scripting, config,
//! and markup languages without external grammars.

use std::fmt::Write as _;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Lang {
    Rust,
    JsTs,
    Python,
    Go,
    CStyle,
    Shell,
    Json,
    Yaml,
    Toml,
    Sql,
    HtmlXml,
    Css,
    Plain,
}

impl Lang {
    fn from_name(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "rs" | "rust" => Self::Rust,
            "js" | "mjs" | "cjs" | "jsx" | "javascript" | "ts" | "tsx"
            | "typescript" => Self::JsTs,
            "py" | "python" => Self::Python,
            "go" | "golang" => Self::Go,
            "c" | "cpp" | "cc" | "cxx" | "h" | "hpp" | "java" | "zig"
            | "swift" | "kt" | "scala" => Self::CStyle,
            "sh" | "bash" | "zsh" | "fish" | "shell" => Self::Shell,
            "json" | "jsonc" => Self::Json,
            "yaml" | "yml" => Self::Yaml,
            "toml" => Self::Toml,
            "sql" => Self::Sql,
            "html" | "htm" | "xml" | "svg" => Self::HtmlXml,
            "css" | "scss" => Self::Css,
            _ => Self::Plain,
        }
    }

    fn supports_slash_comments(self) -> bool {
        matches!(
            self,
            Self::Rust | Self::JsTs | Self::Go | Self::CStyle | Self::Json
        )
    }

    fn supports_block_comments(self) -> bool {
        matches!(
            self,
            Self::Rust
                | Self::JsTs
                | Self::Go
                | Self::CStyle
                | Self::Css
                | Self::Sql
        )
    }

    fn supports_hash_comments(self) -> bool {
        matches!(self, Self::Python | Self::Shell | Self::Yaml | Self::Toml)
    }
}

/// Escape `&`, `<`, `>`, `"`, `'` safely for HTML emission.
pub fn html_escape(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        push_escaped(&mut out, ch);
    }
    out
}

fn push_escaped(out: &mut String, ch: char) {
    match ch {
        '&' => out.push_str("&amp;"),
        '<' => out.push_str("&lt;"),
        '>' => out.push_str("&gt;"),
        '"' => out.push_str("&quot;"),
        '\'' => out.push_str("&#x27;"),
        _ => out.push(ch),
    }
}

fn push_span(out: &mut String, class: &str, text: &str) {
    let _ = write!(out, "<span class=\"{class}\">");
    for ch in text.chars() {
        push_escaped(out, ch);
    }
    out.push_str("</span>");
}

/// Highlight `code` using lexical scanning tailored to `lang`.
pub fn highlight_code(lang: &str, code: &str) -> String {
    let kind = Lang::from_name(lang);
    match kind {
        Lang::Plain => html_escape(code),
        Lang::HtmlXml => highlight_html(code),
        Lang::Css => highlight_css(code),
        _ => highlight_tokens(kind, code),
    }
}

fn highlight_html(src: &str) -> String {
    let mut out = String::with_capacity(src.len() + 64);
    let mut i = 0usize;
    let bytes = src.as_bytes();
    while i < bytes.len() {
        if src[i..].starts_with("<!--") {
            let end = src[i + 4..]
                .find("-->")
                .map_or(src.len(), |pos| i + 4 + pos + 3);
            push_span(&mut out, "hl-cmt", &src[i..end]);
            i = end;
        } else if bytes[i] == b'<' {
            out.push_str("&lt;");
            i += 1;
            if i < bytes.len() && bytes[i] == b'/' {
                out.push('/');
                i += 1;
            }
            let tag_start = i;
            while i < bytes.len()
                && (bytes[i].is_ascii_alphanumeric()
                    || bytes[i] == b'-'
                    || bytes[i] == b':')
            {
                i += 1;
            }
            if i > tag_start {
                push_span(&mut out, "hl-kw", &src[tag_start..i]);
            }
            while i < bytes.len() && bytes[i] != b'>' {
                let b = bytes[i];
                if b == b'"' || b == b'\'' {
                    let quote = b;
                    let start = i;
                    i += 1;
                    while i < bytes.len() && bytes[i] != quote {
                        i += 1;
                    }
                    if i < bytes.len() {
                        i += 1;
                    }
                    push_span(&mut out, "hl-str", &src[start..i]);
                } else if b.is_ascii_alphabetic() {
                    let start = i;
                    while i < bytes.len()
                        && (bytes[i].is_ascii_alphanumeric()
                            || bytes[i] == b'-'
                            || bytes[i] == b':')
                    {
                        i += 1;
                    }
                    push_span(&mut out, "hl-attr", &src[start..i]);
                } else {
                    let ch = src[i..].chars().next().unwrap();
                    push_escaped(&mut out, ch);
                    i += ch.len_utf8();
                }
            }
            if i < bytes.len() && bytes[i] == b'>' {
                out.push_str("&gt;");
                i += 1;
            }
        } else {
            let ch = src[i..].chars().next().unwrap();
            push_escaped(&mut out, ch);
            i += ch.len_utf8();
        }
    }
    out
}

fn highlight_css(src: &str) -> String {
    let mut out = String::with_capacity(src.len() + 64);
    let mut i = 0usize;
    let bytes = src.as_bytes();
    while i < bytes.len() {
        if src[i..].starts_with("/*") {
            let end =
                src[i + 2..].find("*/").map_or(src.len(), |p| i + 2 + p + 2);
            push_span(&mut out, "hl-cmt", &src[i..end]);
            i = end;
        } else if bytes[i] == b'"' || bytes[i] == b'\'' {
            let end = scan_quoted(src, i, bytes[i]);
            push_span(&mut out, "hl-str", &src[i..end]);
            i = end;
        } else if bytes[i] == b'#'
            && bytes.get(i + 1).is_some_and(u8::is_ascii_hexdigit)
        {
            let start = i;
            i += 1;
            while i < bytes.len() && bytes[i].is_ascii_hexdigit() {
                i += 1;
            }
            push_span(&mut out, "hl-num", &src[start..i]);
        } else if bytes[i].is_ascii_digit() {
            let end = scan_number(src, i);
            push_span(&mut out, "hl-num", &src[i..end]);
            i = end;
        } else if bytes[i].is_ascii_alphabetic() || bytes[i] == b'-' {
            let start = i;
            while i < bytes.len()
                && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'-')
            {
                i += 1;
            }
            let word = &src[start..i];
            if src[i..].trim_start().starts_with(':') {
                push_span(&mut out, "hl-attr", word);
            } else {
                out.push_str(word);
            }
        } else {
            let ch = src[i..].chars().next().unwrap();
            push_escaped(&mut out, ch);
            i += ch.len_utf8();
        }
    }
    out
}

fn highlight_tokens(lang: Lang, src: &str) -> String {
    let mut out = String::with_capacity(src.len() + 64);
    let bytes = src.as_bytes();
    let mut i = 0usize;
    let mut after_def_kw = false;

    while i < bytes.len() {
        let rest = &src[i..];

        // Line comments
        if (lang.supports_slash_comments() && rest.starts_with("//"))
            || (lang == Lang::Sql && rest.starts_with("--"))
            || (lang.supports_hash_comments() && bytes[i] == b'#')
        {
            let end = rest.find('\n').map_or(src.len(), |p| i + p);
            push_span(&mut out, "hl-cmt", &src[i..end]);
            i = end;
            after_def_kw = false;
            continue;
        }

        // Block comments
        if lang.supports_block_comments() && rest.starts_with("/*") {
            let end = rest[2..].find("*/").map_or(src.len(), |p| i + 2 + p + 2);
            push_span(&mut out, "hl-cmt", &src[i..end]);
            i = end;
            after_def_kw = false;
            continue;
        }

        // Quoted strings and chars (distinguish Rust lifetimes like `'a`)
        if bytes[i] == b'"'
            || bytes[i] == b'\''
            || (lang == Lang::JsTs && bytes[i] == b'`')
        {
            if lang == Lang::Rust && bytes[i] == b'\'' {
                let is_char_lit = scan_rust_char_end(src, i);
                if let Some(end) = is_char_lit {
                    push_span(&mut out, "hl-str", &src[i..end]);
                    i = end;
                    after_def_kw = false;
                    continue;
                }
                // Lifetime identifier like `'static` / `'a`
                let start = i;
                i += 1;
                while i < bytes.len() && is_ident_continue(bytes[i]) {
                    i += 1;
                }
                push_span(&mut out, "hl-ty", &src[start..i]);
                after_def_kw = false;
                continue;
            }
            let end = scan_quoted(src, i, bytes[i]);
            let tok = &src[i..end];
            if lang == Lang::Json && src[end..].trim_start().starts_with(':') {
                push_span(&mut out, "hl-attr", tok);
            } else {
                push_span(&mut out, "hl-str", tok);
            }
            i = end;
            after_def_kw = false;
            continue;
        }

        // Numbers
        if bytes[i].is_ascii_digit() {
            let end = scan_number(src, i);
            push_span(&mut out, "hl-num", &src[i..end]);
            i = end;
            after_def_kw = false;
            continue;
        }

        // Identifiers & keywords
        if is_ident_start(bytes[i]) {
            let start = i;
            i += 1;
            while i < bytes.len()
                && (is_ident_continue(bytes[i])
                    || (lang == Lang::Yaml && bytes[i] == b'-'))
            {
                i += 1;
            }
            let word = &src[start..i];
            if lang == Lang::Rust && bytes.get(i) == Some(&b'!') {
                i += 1;
                push_span(&mut out, "hl-fn", &src[start..i]);
                after_def_kw = false;
                continue;
            }

            if is_keyword(lang, word) {
                push_span(&mut out, "hl-kw", word);
                after_def_kw =
                    matches!(word, "fn" | "def" | "func" | "function");
            } else if after_def_kw {
                push_span(&mut out, "hl-fn", word);
                after_def_kw = false;
            } else if matches!(lang, Lang::Yaml | Lang::Toml)
                && src[i..].trim_start().starts_with([':', '='])
            {
                push_span(&mut out, "hl-attr", word);
                after_def_kw = false;
            } else if is_builtin_type(lang, word) {
                push_span(&mut out, "hl-ty", word);
                after_def_kw = false;
            } else if src[i..].starts_with('(')
                && !matches!(lang, Lang::Json | Lang::Yaml | Lang::Toml)
            {
                push_span(&mut out, "hl-fn", word);
                after_def_kw = false;
            } else {
                out.push_str(word);
                after_def_kw = false;
            }
            continue;
        }

        // TOML section headers `[package]`
        if lang == Lang::Toml
            && bytes[i] == b'['
            && let Some(close_rel) = rest.find(']')
            && !rest[..close_rel].contains('\n')
        {
            let end = i + close_rel + 1;
            push_span(&mut out, "hl-kw", &src[i..end]);
            i = end;
            after_def_kw = false;
            continue;
        }

        let ch = rest.chars().next().unwrap();
        push_escaped(&mut out, ch);
        if !ch.is_whitespace() {
            after_def_kw = false;
        }
        i += ch.len_utf8();
    }

    out
}

fn scan_rust_char_end(src: &str, start: usize) -> Option<usize> {
    let mut chars = src[start + 1..].char_indices();
    let (_, first) = chars.next()?;
    if first == '\\' {
        let (_, _escaped) = chars.next()?;
        for (idx, c) in chars {
            if c == '\'' {
                return Some(start + 1 + idx + 1);
            }
            if c == '\n' {
                return None;
            }
        }
        None
    } else if first == '\'' {
        Some(start + 2)
    } else {
        let (idx, second) = chars.next()?;
        if second == '\'' {
            Some(start + 1 + idx + 1)
        } else {
            None
        }
    }
}

fn scan_quoted(src: &str, start: usize, quote: u8) -> usize {
    let bytes = src.as_bytes();
    let mut i = start + 1;
    let allow_newline = quote == b'`' || bytes.len() > i;
    let _ = allow_newline;
    while i < bytes.len() {
        if bytes[i] == b'\\' {
            i = (i + 2).min(bytes.len());
            continue;
        }
        if bytes[i] == quote {
            return i + 1;
        }
        if bytes[i] == b'\n' && quote != b'`' {
            return i;
        }
        i += 1;
    }
    bytes.len()
}

fn scan_number(src: &str, start: usize) -> usize {
    let bytes = src.as_bytes();
    let mut i = start;
    while i < bytes.len()
        && (bytes[i].is_ascii_alphanumeric()
            || bytes[i] == b'_'
            || bytes[i] == b'.')
    {
        i += 1;
    }
    i
}

const fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}

const fn is_ident_continue(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn is_keyword(lang: Lang, w: &str) -> bool {
    match lang {
        Lang::Rust => matches!(
            w,
            "fn" | "let"
                | "mut"
                | "const"
                | "static"
                | "pub"
                | "crate"
                | "mod"
                | "use"
                | "struct"
                | "enum"
                | "trait"
                | "impl"
                | "for"
                | "in"
                | "while"
                | "loop"
                | "if"
                | "else"
                | "match"
                | "return"
                | "break"
                | "continue"
                | "async"
                | "await"
                | "move"
                | "ref"
                | "type"
                | "where"
                | "as"
                | "unsafe"
                | "dyn"
                | "true"
                | "false"
                | "self"
                | "Self"
        ),
        Lang::JsTs => matches!(
            w,
            "const"
                | "let"
                | "var"
                | "function"
                | "return"
                | "if"
                | "else"
                | "for"
                | "while"
                | "switch"
                | "case"
                | "break"
                | "continue"
                | "import"
                | "export"
                | "from"
                | "default"
                | "class"
                | "extends"
                | "interface"
                | "type"
                | "async"
                | "await"
                | "new"
                | "try"
                | "catch"
                | "throw"
                | "true"
                | "false"
                | "null"
                | "undefined"
        ),
        Lang::Python => matches!(
            w,
            "def"
                | "class"
                | "return"
                | "if"
                | "elif"
                | "else"
                | "for"
                | "in"
                | "while"
                | "import"
                | "from"
                | "as"
                | "with"
                | "try"
                | "except"
                | "finally"
                | "raise"
                | "async"
                | "await"
                | "pass"
                | "break"
                | "continue"
                | "lambda"
                | "and"
                | "or"
                | "not"
                | "True"
                | "False"
                | "None"
        ),
        Lang::Go => matches!(
            w,
            "package"
                | "import"
                | "func"
                | "var"
                | "const"
                | "type"
                | "struct"
                | "interface"
                | "return"
                | "if"
                | "else"
                | "for"
                | "range"
                | "switch"
                | "case"
                | "default"
                | "go"
                | "defer"
                | "select"
                | "chan"
                | "map"
                | "true"
                | "false"
                | "nil"
        ),
        Lang::CStyle => matches!(
            w,
            "if" | "else"
                | "for"
                | "while"
                | "do"
                | "switch"
                | "case"
                | "break"
                | "continue"
                | "return"
                | "struct"
                | "enum"
                | "class"
                | "public"
                | "private"
                | "const"
                | "static"
                | "void"
                | "typedef"
                | "sizeof"
                | "import"
                | "package"
                | "true"
                | "false"
                | "null"
                | "nullptr"
        ),
        Lang::Shell => matches!(
            w,
            "if" | "then"
                | "else"
                | "elif"
                | "fi"
                | "for"
                | "in"
                | "do"
                | "done"
                | "while"
                | "case"
                | "esac"
                | "function"
                | "export"
                | "local"
                | "return"
                | "exit"
                | "echo"
                | "set"
        ),
        Lang::Json | Lang::Yaml | Lang::Toml => {
            matches!(w, "true" | "false" | "null")
        }
        Lang::Sql => matches!(
            w.to_ascii_uppercase().as_str(),
            "SELECT"
                | "FROM"
                | "WHERE"
                | "INSERT"
                | "INTO"
                | "VALUES"
                | "UPDATE"
                | "SET"
                | "DELETE"
                | "CREATE"
                | "TABLE"
                | "INDEX"
                | "JOIN"
                | "LEFT"
                | "RIGHT"
                | "INNER"
                | "ON"
                | "GROUP"
                | "BY"
                | "ORDER"
                | "LIMIT"
                | "AS"
                | "AND"
                | "OR"
                | "NOT"
                | "NULL"
        ),
        Lang::HtmlXml | Lang::Css | Lang::Plain => false,
    }
}

fn is_builtin_type(lang: Lang, w: &str) -> bool {
    match lang {
        Lang::Rust => matches!(
            w,
            "u8" | "u16"
                | "u32"
                | "u64"
                | "u128"
                | "usize"
                | "i8"
                | "i16"
                | "i32"
                | "i64"
                | "i128"
                | "isize"
                | "f32"
                | "f64"
                | "bool"
                | "char"
                | "str"
                | "String"
                | "Vec"
                | "Option"
                | "Result"
                | "Box"
                | "Arc"
        ),
        Lang::JsTs => {
            matches!(w, "string" | "number" | "boolean" | "void" | "any")
        }
        Lang::Go => {
            matches!(
                w,
                "string"
                    | "int"
                    | "int64"
                    | "uint64"
                    | "bool"
                    | "byte"
                    | "error"
            )
        }
        Lang::CStyle => {
            matches!(
                w,
                "int"
                    | "long"
                    | "char"
                    | "bool"
                    | "float"
                    | "double"
                    | "String"
            )
        }
        _ => false,
    }
}
