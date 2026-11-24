//! Two-phase block Markdown parser and renderer: CommonMark + GFM tables,
//! task lists, footnotes, CJK/ASCII heading anchors, and native embedded HTML.

use crate::highlight::{highlight_code, html_escape};
use crate::inline::{
    InlineCtx, RefDef, heading_plain_text, render_inline, slugify_heading,
};
use render_api::{HeadingEntry, MarkdownOptions, RenderedDocument};
use std::collections::HashMap;
use std::fmt::Write as _;

struct DocParser {
    opts: MarkdownOptions,
    refs: HashMap<String, RefDef>,
    footnotes: HashMap<String, String>,
    headings: Vec<HeadingEntry>,
    heading_counts: HashMap<String, usize>,
    fn_order: Vec<String>,
    fn_indices: HashMap<String, usize>,
}

impl DocParser {
    fn new(opts: MarkdownOptions) -> Self {
        Self {
            opts,
            refs: HashMap::new(),
            footnotes: HashMap::new(),
            headings: Vec::new(),
            heading_counts: HashMap::new(),
            fn_order: Vec::new(),
            fn_indices: HashMap::new(),
        }
    }

    fn render_document(&mut self, src: &str) -> RenderedDocument {
        let normalized = src.replace("\r\n", "\n").replace('\r', "\n");
        let lines: Vec<&str> = normalized.lines().collect();
        let body_lines = self.extract_definitions(&lines);

        let mut html = String::with_capacity(normalized.len() + 256);
        self.render_blocks(&body_lines, &mut html);

        if !self.fn_order.is_empty() {
            html.push_str("<section class=\"md-footnotes\">\n<ol>\n");
            let order = self.fn_order.clone();
            for (idx0, id) in order.iter().enumerate() {
                let n = idx0 + 1;
                let text = self.footnotes.get(id).cloned().unwrap_or_default();
                let rendered = self.render_inline_text(&text);
                let _ = writeln!(
                    html,
                    "<li id=\"fn-{n}\">{rendered} <a href=\"#fnref-{n}\">&#8617;</a></li>"
                );
            }
            html.push_str("</ol>\n</section>\n");
        }

        if let Some(sanitizer) = &self.opts.sanitizer {
            html = sanitizer(&html);
        }

        RenderedDocument {
            html,
            headings: std::mem::take(&mut self.headings),
        }
    }

    fn render_inline_text(&mut self, src: &str) -> String {
        let mut ctx = InlineCtx {
            allow_raw_html: self.opts.allow_raw_html,
            refs: &self.refs,
            footnotes: &self.footnotes,
            fn_order: &mut self.fn_order,
            fn_indices: &mut self.fn_indices,
        };
        render_inline(src, &mut ctx)
    }

    /// Pass 1: Extract footnote definitions (`[^id]: ...`) and link ref defs
    /// (`[id]: url "title"`) outside fenced code blocks.
    fn extract_definitions<'a>(&mut self, lines: &[&'a str]) -> Vec<&'a str> {
        let mut kept = Vec::with_capacity(lines.len());
        let mut i = 0usize;
        let mut fence: Option<(char, usize)> = None;

        while i < lines.len() {
            let line = lines[i];
            if let Some((fch, flen)) = fence {
                kept.push(line);
                if is_fence_close(line, fch, flen) {
                    fence = None;
                }
                i += 1;
                continue;
            }
            if let Some((fch, flen, _)) = parse_fence_open(line) {
                fence = Some((fch, flen));
                kept.push(line);
                i += 1;
                continue;
            }

            if let Some((id, first_body)) = parse_footnote_def(line) {
                let mut text = first_body.to_owned();
                i += 1;
                while i < lines.len() {
                    let next = lines[i];
                    if next.starts_with("    ") || next.starts_with('\t') {
                        text.push(' ');
                        text.push_str(next.trim());
                        i += 1;
                    } else if next.trim().is_empty()
                        && lines.get(i + 1).is_some_and(|l| {
                            l.starts_with("    ") || l.starts_with('\t')
                        })
                    {
                        i += 1;
                    } else {
                        break;
                    }
                }
                self.footnotes.insert(id.to_owned(), text);
                continue;
            }

            if let Some((id, def)) = parse_link_ref_def(line) {
                self.refs.entry(id).or_insert(def);
                i += 1;
                continue;
            }

            kept.push(line);
            i += 1;
        }
        kept
    }

    fn render_blocks(&mut self, lines: &[&str], out: &mut String) {
        let mut i = 0usize;
        while i < lines.len() {
            let line = lines[i];
            if line.trim().is_empty() {
                i += 1;
                continue;
            }

            // 1. Fenced code block
            if let Some((fch, flen, lang)) = parse_fence_open(line) {
                i += 1;
                let mut code_lines = Vec::new();
                while i < lines.len() {
                    if is_fence_close(lines[i], fch, flen) {
                        i += 1;
                        break;
                    }
                    code_lines.push(lines[i]);
                    i += 1;
                }
                let code = code_lines.join("\n");
                let body = highlight_code(&lang, &code);
                if lang.is_empty() {
                    let _ = writeln!(out, "<pre><code>{body}</code></pre>");
                } else {
                    let _ = writeln!(
                        out,
                        "<pre><code class=\"language-{}\">{body}</code></pre>",
                        html_escape(&lang)
                    );
                }
                continue;
            }

            // 2. ATX heading
            if let Some((level, content)) = parse_atx_heading(line) {
                self.emit_heading(level, content, out);
                i += 1;
                continue;
            }

            // 3. Thematic break (check before list since `***`/`---` overlap list markers)
            if is_thematic_break(line) {
                out.push_str("<hr>\n");
                i += 1;
                continue;
            }

            // 4. Block HTML passthrough
            if self.opts.allow_raw_html
                && let Some(consumed) =
                    self.try_raw_html_block(&lines[i..], out)
            {
                i += consumed;
                continue;
            }

            // 5. Blockquote
            if line.trim_start().starts_with('>') {
                let (quote_lines, consumed) = collect_blockquote(&lines[i..]);
                let refs: Vec<&str> =
                    quote_lines.iter().map(String::as_str).collect();
                out.push_str("<blockquote>\n");
                self.render_blocks(&refs, out);
                out.push_str("</blockquote>\n");
                i += consumed;
                continue;
            }

            // 6. List (ordered or unordered, including GFM task items)
            if parse_list_item(line).is_some() {
                let consumed = self.render_list(&lines[i..], out);
                i += consumed;
                continue;
            }

            // 7. GFM pipe table
            if i + 1 < lines.len()
                && let Some(aligns) = parse_table_delim(lines[i + 1])
            {
                let headers = split_table_row(lines[i]);
                if !headers.is_empty() && headers.len() == aligns.len() {
                    let consumed = self.render_table(
                        &headers,
                        &aligns,
                        &lines[i + 2..],
                        out,
                    );
                    i += 2 + consumed;
                    continue;
                }
            }

            // 8. Indented code block (4 spaces or 1 tab)
            if is_indented_code(line) {
                let mut code = Vec::new();
                while i < lines.len()
                    && (lines[i].trim().is_empty()
                        || is_indented_code(lines[i]))
                {
                    code.push(strip_indent_code(lines[i]));
                    i += 1;
                }
                while code.last().is_some_and(|l| l.is_empty()) {
                    code.pop();
                }
                let escaped = html_escape(&code.join("\n"));
                let _ = writeln!(out, "<pre><code>{escaped}</code></pre>");
                continue;
            }

            // 9. Paragraph or Setext heading
            let mut para = Vec::new();
            let mut setext_level = None;
            while i < lines.len() {
                let cur = lines[i];
                if cur.trim().is_empty() {
                    break;
                }
                if !para.is_empty()
                    && let Some(lvl) = parse_setext_underline(cur)
                {
                    setext_level = Some(lvl);
                    i += 1;
                    break;
                }
                if !para.is_empty()
                    && (parse_fence_open(cur).is_some()
                        || parse_atx_heading(cur).is_some()
                        || cur.trim_start().starts_with('>')
                        || parse_list_item(cur).is_some()
                        || is_thematic_break(cur)
                        || (self.opts.allow_raw_html
                            && starts_html_block_line(cur)))
                {
                    break;
                }
                para.push(cur.trim());
                i += 1;
            }

            let joined = para.join("\n");
            if let Some(level) = setext_level {
                self.emit_heading(level, &joined, out);
            } else {
                let inline = self.render_inline_text(&joined);
                let _ = writeln!(out, "<p>{inline}</p>");
            }
        }
    }

    fn emit_heading(&mut self, level: u8, raw_text: &str, out: &mut String) {
        let plain = heading_plain_text(raw_text);
        let inline = self.render_inline_text(raw_text);
        if self.opts.heading_anchors {
            let id = slugify_heading(&plain, &mut self.heading_counts);
            self.headings.push(HeadingEntry {
                level,
                text: plain,
                id: id.clone(),
            });
            let _ = writeln!(
                out,
                "<h{level} id=\"{id}\">{inline}<a class=\"md-anchor\" href=\"#{id}\">#</a></h{level}>"
            );
        } else {
            let _ = writeln!(out, "<h{level}>{inline}</h{level}>");
        }
    }

    fn try_raw_html_block(
        &self,
        lines: &[&str],
        out: &mut String,
    ) -> Option<usize> {
        let first = lines.first()?.trim_start();
        if !first.starts_with('<') {
            return None;
        }
        if first.starts_with("<!--") {
            let mut idx = 0usize;
            while idx < lines.len() {
                out.push_str(lines[idx]);
                out.push('\n');
                if lines[idx].contains("-->") {
                    return Some(idx + 1);
                }
                idx += 1;
            }
            return Some(lines.len());
        }

        if !starts_html_block_line(first) {
            return None;
        }

        let mut idx = 0usize;
        while idx < lines.len() {
            if lines[idx].trim().is_empty() {
                break;
            }
            out.push_str(lines[idx]);
            out.push('\n');
            idx += 1;
        }
        Some(idx.max(1))
    }

    fn render_list(&mut self, lines: &[&str], out: &mut String) -> usize {
        let first =
            parse_list_item(lines[0]).expect("caller verified list item");
        let base_indent = first.indent;
        let ordered = first.ordered;
        let start_num = first.start;

        if ordered {
            if start_num == 1 {
                out.push_str("<ol>\n");
            } else {
                let _ = writeln!(out, "<ol start=\"{start_num}\">");
            }
        } else {
            out.push_str("<ul>\n");
        }

        let mut i = 0usize;
        while i < lines.len() {
            if lines[i].trim().is_empty() {
                let next_is_same = lines
                    .get(i + 1)
                    .and_then(|l| parse_list_item(l))
                    .is_some_and(|it| {
                        it.indent == base_indent && it.ordered == ordered
                    });
                if next_is_same {
                    i += 1;
                    continue;
                }
                break;
            }
            let Some(item) = parse_list_item(lines[i]) else {
                break;
            };
            if item.indent < base_indent || item.ordered != ordered {
                break;
            }
            if item.indent > base_indent {
                break;
            }

            let content_indent = item.content_col;
            i += 1;
            let mut sub_lines: Vec<&str> = Vec::new();
            while i < lines.len() {
                let cur = lines[i];
                if cur.trim().is_empty() {
                    if lines
                        .get(i + 1)
                        .is_some_and(|n| leading_spaces(n) >= content_indent)
                    {
                        sub_lines.push("");
                        i += 1;
                        continue;
                    }
                    break;
                }
                if parse_list_item(cur)
                    .is_some_and(|it| it.indent <= base_indent)
                {
                    break;
                }
                if leading_spaces(cur) >= content_indent
                    || parse_list_item(cur).is_some()
                {
                    let trimmed = trim_indent_up_to(cur, content_indent);
                    sub_lines.push(trimmed);
                    i += 1;
                } else {
                    break;
                }
            }

            let (task_checked, item_text) = parse_task_prefix(item.text);
            let inline = self.render_inline_text(item_text);

            let mut nested_html = String::new();
            if !sub_lines.is_empty() {
                self.render_blocks(&sub_lines, &mut nested_html);
            }

            if let Some(checked) = task_checked {
                let chk = if checked {
                    " checked disabled"
                } else {
                    " disabled"
                };
                let _ = write!(
                    out,
                    "<li class=\"md-task\"><input type=\"checkbox\"{chk}> {inline}"
                );
            } else {
                let _ = write!(out, "<li>{inline}");
            }
            if !nested_html.is_empty() {
                out.push('\n');
                out.push_str(&nested_html);
            }
            out.push_str("</li>\n");
        }

        out.push_str(if ordered { "</ol>\n" } else { "</ul>\n" });
        i
    }

    fn render_table(
        &mut self,
        headers: &[String],
        aligns: &[Option<&'static str>],
        body_lines: &[&str],
        out: &mut String,
    ) -> usize {
        out.push_str("<table>\n<thead>\n<tr>");
        for (cell, align) in headers.iter().zip(aligns.iter()) {
            let rendered = self.render_inline_text(cell);
            match align {
                Some(a) => {
                    let _ = write!(
                        out,
                        "<th style=\"text-align:{a}\">{rendered}</th>"
                    );
                }
                None => {
                    let _ = write!(out, "<th>{rendered}</th>");
                }
            }
        }
        out.push_str("</tr>\n</thead>\n");

        let mut consumed = 0usize;
        let mut rows = Vec::new();
        while consumed < body_lines.len() {
            let line = body_lines[consumed];
            if line.trim().is_empty() || !line.contains('|') {
                break;
            }
            let cells = split_table_row(line);
            if cells.is_empty() {
                break;
            }
            rows.push(cells);
            consumed += 1;
        }

        if !rows.is_empty() {
            out.push_str("<tbody>\n");
            for row in rows {
                out.push_str("<tr>");
                for (idx, align) in aligns.iter().enumerate() {
                    let raw = row.get(idx).map_or("", String::as_str);
                    let rendered = self.render_inline_text(raw);
                    match align {
                        Some(a) => {
                            let _ = write!(
                                out,
                                "<td style=\"text-align:{a}\">{rendered}</td>"
                            );
                        }
                        None => {
                            let _ = write!(out, "<td>{rendered}</td>");
                        }
                    }
                }
                out.push_str("</tr>\n");
            }
            out.push_str("</tbody>\n");
        }

        out.push_str("</table>\n");
        consumed
    }
}

/// Public entry point called by `DefaultRenderer::render_markdown`.
pub fn render_markdown_document(
    src: &str,
    opts: &MarkdownOptions,
) -> RenderedDocument {
    DocParser::new(opts.clone()).render_document(src)
}

fn leading_spaces(line: &str) -> usize {
    let mut cols = 0usize;
    for b in line.bytes() {
        match b {
            b' ' => cols += 1,
            b'\t' => cols += 4,
            _ => break,
        }
    }
    cols
}

fn trim_indent_up_to(line: &str, max_cols: usize) -> &str {
    let mut cols = 0usize;
    let mut byte_idx = 0usize;
    for b in line.bytes() {
        if cols >= max_cols {
            break;
        }
        match b {
            b' ' => {
                cols += 1;
                byte_idx += 1;
            }
            b'\t' => {
                cols += 4;
                byte_idx += 1;
            }
            _ => break,
        }
    }
    &line[byte_idx..]
}

fn parse_fence_open(line: &str) -> Option<(char, usize, String)> {
    let trimmed = line.trim_start();
    if leading_spaces(line) > 3 {
        return None;
    }
    let ch = trimmed.chars().next()?;
    if ch != '`' && ch != '~' {
        return None;
    }
    let count = trimmed.chars().take_while(|&c| c == ch).count();
    if count < 3 {
        return None;
    }
    let info = trimmed[count..].trim();
    if ch == '`' && info.contains('`') {
        return None;
    }
    let lang = info.split_whitespace().next().unwrap_or("").to_string();
    Some((ch, count, lang))
}

fn is_fence_close(line: &str, fence_ch: char, open_len: usize) -> bool {
    let trimmed = line.trim();
    let count = trimmed.chars().take_while(|&c| c == fence_ch).count();
    count >= open_len && trimmed[count..].is_empty()
}

fn parse_atx_heading(line: &str) -> Option<(u8, &str)> {
    let trimmed = line.trim_start();
    if leading_spaces(line) > 3 || !trimmed.starts_with('#') {
        return None;
    }
    let hashes = trimmed.bytes().take_while(|&b| b == b'#').count();
    if !(1..=6).contains(&hashes) {
        return None;
    }
    let rest = &trimmed[hashes..];
    if !rest.is_empty() && !rest.starts_with([' ', '\t']) {
        return None;
    }
    let mut content = rest.trim();
    let without_hashes = content.trim_end_matches('#');
    if without_hashes.is_empty() || without_hashes.ends_with([' ', '\t']) {
        content = without_hashes.trim();
    }
    Some((hashes as u8, content))
}

fn parse_setext_underline(line: &str) -> Option<u8> {
    if leading_spaces(line) > 3 {
        return None;
    }
    let t = line.trim();
    if t.is_empty() {
        return None;
    }
    if t.bytes().all(|b| b == b'=') {
        Some(1)
    } else if t.bytes().all(|b| b == b'-') {
        Some(2)
    } else {
        None
    }
}

fn is_thematic_break(line: &str) -> bool {
    if leading_spaces(line) > 3 {
        return None::<()>.is_some();
    }
    let compact: Vec<u8> =
        line.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    if compact.len() < 3 {
        return false;
    }
    let first = compact[0];
    matches!(first, b'-' | b'*' | b'_') && compact.iter().all(|&b| b == first)
}

fn starts_html_block_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    let Some(after) = trimmed.strip_prefix('<') else {
        return false;
    };
    if after.starts_with("!--") {
        return true;
    }
    let tag_part = after.strip_prefix('/').unwrap_or(after);
    let end = tag_part
        .bytes()
        .position(|b| !b.is_ascii_alphanumeric())
        .unwrap_or(tag_part.len());
    if end == 0 {
        return false;
    }
    let tag = tag_part[..end].to_ascii_lowercase();
    matches!(
        tag.as_str(),
        "details"
            | "summary"
            | "figure"
            | "figcaption"
            | "table"
            | "thead"
            | "tbody"
            | "tfoot"
            | "tr"
            | "th"
            | "td"
            | "div"
            | "section"
            | "article"
            | "aside"
            | "header"
            | "footer"
            | "nav"
            | "main"
            | "pre"
            | "blockquote"
            | "ul"
            | "ol"
            | "li"
            | "dl"
            | "dt"
            | "dd"
            | "video"
            | "audio"
            | "source"
            | "iframe"
            | "picture"
            | "img"
            | "canvas"
            | "svg"
            | "hr"
            | "p"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
            | "style"
    )
}

fn collect_blockquote(lines: &[&str]) -> (Vec<String>, usize) {
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let trimmed = lines[i].trim_start();
        if let Some(after) = trimmed.strip_prefix('>') {
            let line = after.strip_prefix(' ').unwrap_or(after);
            out.push(line.to_owned());
            i += 1;
        } else {
            break;
        }
    }
    (out, i)
}

struct ListItem<'a> {
    indent: usize,
    ordered: bool,
    start: usize,
    content_col: usize,
    text: &'a str,
}

fn parse_list_item(line: &str) -> Option<ListItem<'_>> {
    let indent = leading_spaces(line);
    let rest = &line[line.len() - line.trim_start().len()..];
    if let Some(after) = rest
        .strip_prefix("- ")
        .or_else(|| rest.strip_prefix("* "))
        .or_else(|| rest.strip_prefix("+ "))
    {
        return Some(ListItem {
            indent,
            ordered: false,
            start: 1,
            content_col: indent + 2,
            text: after.trim_start(),
        });
    }
    if rest == "-" || rest == "*" || rest == "+" {
        return Some(ListItem {
            indent,
            ordered: false,
            start: 1,
            content_col: indent + 2,
            text: "",
        });
    }

    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    if digits > 0 && digits <= 9 {
        let delim = rest.as_bytes().get(digits).copied();
        if matches!(delim, Some(b'.' | b')'))
            && rest.as_bytes().get(digits + 1) == Some(&b' ')
        {
            let start = rest[..digits].parse::<usize>().ok()?;
            return Some(ListItem {
                indent,
                ordered: true,
                start,
                content_col: indent + digits + 2,
                text: rest[digits + 2..].trim_start(),
            });
        }
    }
    None
}

fn parse_task_prefix(text: &str) -> (Option<bool>, &str) {
    if let Some(rest) = text.strip_prefix("[ ] ") {
        (Some(false), rest)
    } else if let Some(rest) = text
        .strip_prefix("[x] ")
        .or_else(|| text.strip_prefix("[X] "))
    {
        (Some(true), rest)
    } else {
        (None, text)
    }
}

fn split_table_row(line: &str) -> Vec<String> {
    let mut trimmed = line.trim();
    if trimmed.starts_with('|') {
        trimmed = &trimmed[1..];
    }
    if trimmed.ends_with('|') && !trimmed.ends_with("\\|") {
        trimmed = &trimmed[..trimmed.len() - 1];
    }
    let mut cells = Vec::new();
    let mut cur = String::new();
    let mut chars = trimmed.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' && chars.peek() == Some(&'|') {
            chars.next();
            cur.push('|');
        } else if c == '|' {
            cells.push(cur.trim().to_owned());
            cur.clear();
        } else {
            cur.push(c);
        }
    }
    cells.push(cur.trim().to_owned());
    cells
}

fn parse_table_delim(line: &str) -> Option<Vec<Option<&'static str>>> {
    if !line.contains('|') && !line.contains('-') {
        return None;
    }
    let cells = split_table_row(line);
    if cells.is_empty() {
        return None;
    }
    let mut aligns = Vec::with_capacity(cells.len());
    for cell in cells {
        let c = cell.trim();
        if c.is_empty() || !c.bytes().all(|b| matches!(b, b'-' | b':')) {
            return None;
        }
        let dashes = c.bytes().filter(|&b| b == b'-').count();
        if dashes < 1 {
            return None;
        }
        let align = match (c.starts_with(':'), c.ends_with(':')) {
            (true, true) => Some("center"),
            (true, false) => Some("left"),
            (false, true) => Some("right"),
            (false, false) => None,
        };
        aligns.push(align);
    }
    Some(aligns)
}

fn parse_footnote_def(line: &str) -> Option<(&str, &str)> {
    let trimmed = line.trim_start();
    let after = trimmed.strip_prefix("[^")?;
    let close = after.find("]:")?;
    let id = &after[..close];
    if id.is_empty() || id.contains(char::is_whitespace) {
        return None;
    }
    Some((id, after[close + 2..].trim()))
}

fn parse_link_ref_def(line: &str) -> Option<(String, RefDef)> {
    let trimmed = line.trim_start();
    let after = trimmed.strip_prefix('[')?;
    if after.starts_with('^') {
        return None;
    }
    let close = after.find("]:")?;
    let id = after[..close].trim().to_ascii_lowercase();
    if id.is_empty() {
        return None;
    }
    let rest = after[close + 2..].trim();
    if rest.is_empty() {
        return None;
    }
    let mut parts = rest.splitn(2, char::is_whitespace);
    let url = parts.next()?.trim_matches(['<', '>']).to_owned();
    let title = parts.next().map(str::trim).and_then(|t| {
        t.strip_prefix('"')
            .and_then(|s| s.strip_suffix('"'))
            .or_else(|| t.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')))
            .map(str::to_owned)
    });
    Some((id, RefDef { url, title }))
}

fn is_indented_code(line: &str) -> bool {
    line.starts_with("    ") || line.starts_with('\t')
}

fn strip_indent_code(line: &str) -> &str {
    if let Some(rest) = line.strip_prefix("    ") {
        rest
    } else {
        line.strip_prefix('\t').unwrap_or_default()
    }
}
