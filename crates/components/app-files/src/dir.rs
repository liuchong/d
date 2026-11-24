//! Minimalist white directory listing and `index.html` serving.

use crate::file::serve_file;
use crate::path::{PathResolution, resolve_path};
use files_api::FileCategory;
use files_api::shell::{
    format_file_size, format_utc_datetime, html_escape, percent_encode_path,
    render_white_document,
};
use http_api::{HttpResponse, RequestMeta, StatusCode};
use std::path::Path;
use std::time::SystemTime;
use tokio::fs;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortBy {
    Name,
    Size,
    Time,
    Type,
}

impl SortBy {
    pub fn parse(raw: Option<&str>) -> Self {
        match raw {
            Some("size") => Self::Size,
            Some("time") => Self::Time,
            Some("type") => Self::Type,
            _ => Self::Name,
        }
    }
}

struct DirEntryItem {
    name: String,
    rel_url_path: String,
    is_dir: bool,
    size: u64,
    modified: Option<SystemTime>,
    category: FileCategory,
    ext: String,
}

/// Serve sanitized `index.html` when present and not bypassed via `?listing=true`.
pub async fn try_serve_index(
    root: &Path,
    dir_path: &Path,
    req: &RequestMeta,
    is_head: bool,
) -> Option<HttpResponse> {
    if req.query_param("listing").as_deref() == Some("true") {
        return None;
    }
    let rel_dir = dir_path.strip_prefix(root).ok()?;
    let idx_rel = if rel_dir.as_os_str().is_empty() {
        "/index.html".to_string()
    } else {
        format!(
            "/{}/index.html",
            rel_dir.to_string_lossy().replace('\\', "/")
        )
    };
    let PathResolution::Resolved(idx_path) = resolve_path(root, &idx_rel).await
    else {
        return None;
    };
    let meta = fs::metadata(&idx_path).await.ok()?;
    if !meta.is_file() {
        return None;
    }
    Some(serve_file(&idx_path, &meta, req, is_head).await)
}

/// Serve the minimalist white directory listing page for `dir_path`.
pub async fn serve_directory(
    rel_path: &str,
    dir_path: &Path,
    req: &RequestMeta,
    allow_hidden: bool,
    is_head: bool,
) -> HttpResponse {
    if is_head {
        return HttpResponse::empty(StatusCode::OK)
            .with_header("Content-Type", "text/html; charset=utf-8");
    }

    let query_hidden =
        req.query_param("hidden").and_then(|v| match v.as_str() {
            "true" | "1" => Some(true),
            "false" | "0" => Some(false),
            _ => None,
        });
    let show_hidden = if allow_hidden {
        query_hidden.unwrap_or(true)
    } else {
        false
    };

    let sort_by = SortBy::parse(req.query_param("sort").as_deref());
    let desc = req.query_param("order").as_deref() == Some("desc");

    let mut read_dir = match fs::read_dir(dir_path).await {
        Ok(rd) => rd,
        Err(_) => {
            return HttpResponse::text(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to read directory",
            );
        }
    };

    let clean_rel = rel_path.trim_matches('/');
    let mut entries = Vec::new();
    while let Ok(Some(entry)) = read_dir.next_entry().await {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !show_hidden && name.starts_with('.') {
            continue;
        }
        let Ok(meta) = entry.metadata().await else {
            continue;
        };
        let is_dir = meta.is_dir();
        let ext = if is_dir {
            String::new()
        } else {
            Path::new(&name)
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_string()
        };
        let category = if is_dir {
            FileCategory::Directory
        } else {
            FileCategory::from_extension(&ext)
        };
        let child_rel = if clean_rel.is_empty() {
            name.clone()
        } else {
            format!("{clean_rel}/{name}")
        };
        entries.push(DirEntryItem {
            name,
            rel_url_path: percent_encode_path(&child_rel),
            is_dir,
            size: if is_dir { 0 } else { meta.len() },
            modified: meta.modified().ok(),
            category,
            ext,
        });
    }

    entries.sort_by(|a, b| {
        let dir_ord = b.is_dir.cmp(&a.is_dir);
        if dir_ord != std::cmp::Ordering::Equal {
            return dir_ord;
        }
        let ord = match sort_by {
            SortBy::Name => a
                .name
                .to_ascii_lowercase()
                .cmp(&b.name.to_ascii_lowercase()),
            SortBy::Size => a.size.cmp(&b.size),
            SortBy::Time => a.modified.cmp(&b.modified),
            SortBy::Type => {
                a.category.label().cmp(b.category.label()).then_with(|| {
                    a.name
                        .to_ascii_lowercase()
                        .cmp(&b.name.to_ascii_lowercase())
                })
            }
        };
        if desc { ord.reverse() } else { ord }
    });

    let display_dir = if clean_rel.is_empty() {
        "/".to_string()
    } else {
        format!("/{clean_rel}/")
    };

    let breadcrumb = build_breadcrumb(clean_rel);
    let hidden_toggle = if allow_hidden {
        if show_hidden {
            " | <a href=\"?hidden=false\">Hide dotfiles</a>"
        } else {
            " | <a href=\"?hidden=true\">Show dotfiles</a>"
        }
    } else {
        ""
    };

    let mut rows = String::new();
    if !clean_rel.is_empty() {
        let parent_href = match clean_rel.rsplit_once('/') {
            Some((p, _)) if !p.is_empty() => {
                format!("/{}/", percent_encode_path(p))
            }
            _ => "/".to_string(),
        };
        rows.push_str(&format!(
            "<tr><td><a href=\"{parent}\">../</a></td><td class=\"d-kind\">dir</td><td class=\"d-num\">-</td><td class=\"d-time\">-</td></tr>\n",
            parent = html_escape(&parent_href)
        ));
    }

    for item in &entries {
        let escaped_name = html_escape(&item.name);
        let base_href = if item.is_dir {
            format!("/{}/", item.rel_url_path)
        } else {
            format!("/{}", item.rel_url_path)
        };
        let is_media_or_script = matches!(
            item.category,
            FileCategory::Image | FileCategory::Video | FileCategory::Audio
        ) || (item.category == FileCategory::Code
            && matches!(
                item.ext.to_ascii_lowercase().as_str(),
                "js" | "mjs" | "cjs"
            ));

        let primary_href = if is_media_or_script {
            format!("{base_href}?view=viewer")
        } else {
            base_href.clone()
        };
        let name_suffix = if item.is_dir { "/" } else { "" };

        let extra_links = if item.is_dir {
            String::new()
        } else if is_media_or_script
            || item.category.is_document_or_code_page(&item.ext)
        {
            format!(
                " <span class=\"d-row-links\">(<a href=\"{raw_href}\">raw</a> &middot; <a href=\"{dl_href}\">dl</a>)</span>",
                raw_href = if is_media_or_script {
                    html_escape(&base_href)
                } else {
                    html_escape(&format!("{base_href}?view=raw"))
                },
                dl_href = html_escape(&format!("{base_href}?view=download"))
            )
        } else {
            format!(
                " <span class=\"d-row-links\">(<a href=\"{dl_href}\">dl</a>)</span>",
                dl_href = html_escape(&format!("{base_href}?view=download"))
            )
        };

        let size_str = if item.is_dir {
            "-".to_string()
        } else {
            format_file_size(item.size)
        };
        let mtime_str = format_utc_datetime(item.modified);
        rows.push_str(&format!(
            "<tr><td><a href=\"{href}\">{name}{name_suffix}</a>{extra_links}</td><td class=\"d-kind\">{kind}</td><td class=\"d-num\">{size}</td><td class=\"d-time\">{mtime}</td></tr>\n",
            href = html_escape(&primary_href),
            name = escaped_name,
            kind = item.category.label(),
            size = html_escape(&size_str),
            mtime = html_escape(&mtime_str),
        ));
    }

    let extra_css = r#"
.d-dir-head { margin: 0 0 12px; font-size: 18px; font-weight: 600; color: #1f2328; }
.d-table { width: 100%; border-collapse: collapse; background: #ffffff; }
.d-table th, .d-table td { padding: 8px 10px; border-bottom: 1px solid #e5e7eb; text-align: left; }
.d-table th { background: #f6f8fa; font-weight: 600; color: #1f2328; font-size: 13px; }
.d-table td.d-kind, .d-table td.d-num, .d-table td.d-time { color: #656d76; font-size: 13px; white-space: nowrap; }
.d-table th.d-num, .d-table td.d-num { text-align: right; }
.d-row-links { color: #656d76; font-size: 12px; margin-left: 6px; }
.d-foot { margin-top: 14px; color: #656d76; font-size: 12px; }
"#;

    let body = format!(
        "<h1 class=\"d-dir-head\">Index of {dir}</h1>\n\
         <div class=\"d-topbar\">\n\
           <div class=\"d-topbar-left\">{breadcrumb}</div>\n\
           <nav class=\"d-actions\">Sort: <a href=\"?sort=name\">Name</a> <a href=\"?sort=size\">Size</a> <a href=\"?sort=time\">Time</a> <a href=\"?sort=type\">Type</a>{hidden_toggle}</nav>\n\
         </div>\n\
         <table class=\"d-table\">\n\
           <thead><tr><th>Name</th><th>Type</th><th class=\"d-num\">Size</th><th>Modified</th></tr></thead>\n\
           <tbody>\n{rows}</tbody>\n\
         </table>\n\
         <footer class=\"d-foot\">{count} entries</footer>",
        dir = html_escape(&display_dir),
        count = entries.len(),
    );

    let title = format!("Index of {display_dir}");
    HttpResponse::html(
        StatusCode::OK,
        render_white_document(&title, extra_css, &body),
    )
}

fn build_breadcrumb(clean_rel: &str) -> String {
    if clean_rel.is_empty() {
        return "<a href=\"/\">root</a>".to_string();
    }
    let mut parts = vec!["<a href=\"/\">root</a>".to_string()];
    let mut acc = String::new();
    for seg in clean_rel.split('/') {
        if seg.is_empty() {
            continue;
        }
        if !acc.is_empty() {
            acc.push('/');
        }
        acc.push_str(seg);
        let href = format!("/{}/", percent_encode_path(&acc));
        parts.push(format!(
            "<a href=\"{href}\">{name}</a>",
            href = html_escape(&href),
            name = html_escape(seg)
        ));
    }
    parts.join(" <span class=\"d-meta\">/</span> ")
}
