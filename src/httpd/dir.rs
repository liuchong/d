//! Directory listing: entries, sorting, breadcrumb and HTML generation.

use axum::{
    http::StatusCode,
    response::{Html, IntoResponse, Response},
};
use std::path::{Path, PathBuf};
use tokio::fs;
use tracing::warn;

use super::html::html_escape;
use super::{RequestQuery, ServerState, SortBy};
use crate::utils::{encode_path, format_http_date, format_size};

/// File information for directory listing
pub(crate) struct FileEntry {
    name: String,
    path: String,
    is_dir: bool,
    size: u64,
    modified_time: Option<std::time::SystemTime>,
    file_type: FileType,
}

#[derive(Debug, Clone)]
pub(crate) enum FileType {
    Directory,
    Image,
    Video,
    Audio,
    Code,
    Text,
    Markdown,
    Org,
    Archive,
    Document,
    Executable,
    Unknown,
}

impl FileType {
    pub(crate) fn from_extension(ext: &str) -> Self {
        match ext.to_lowercase().as_str() {
            "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp" | "svg" | "ico"
            | "tiff" | "tif" | "raw" | "heic" | "avif" => FileType::Image,
            "mp4" | "webm" | "mkv" | "avi" | "mov" | "wmv" | "flv" | "m4v"
            | "3gp" | "ogv" | "mpg" | "mpeg" | "m2v" => FileType::Video,
            "mp3" | "wav" | "ogg" | "flac" | "aac" | "m4a" | "wma" | "opus"
            | "aiff" | "au" => FileType::Audio,
            "rs" | "js" | "ts" | "jsx" | "tsx" | "py" | "java" | "c"
            | "cpp" | "cc" | "cxx" | "h" | "hpp" | "go" | "rb" | "php"
            | "swift" | "kt" | "scala" | "r" | "m" | "mm" | "pl" | "pm"
            | "t" | "sh" | "bash" | "zsh" | "fish" | "ps1" | "bat" | "cmd"
            | "vbs" | "lua" | "elm" | "erl" | "hrl" | "ex" | "exs" | "fs"
            | "fsx" | "fsi" | "ml" | "mli" | "hs" | "lhs" | "clj" | "cljs"
            | "cljc" | "edn" | "coffee" | "litcoffee" | "cr" | "dart"
            | "groovy" | "gvy" | "gy" | "gsh" | "p6" | "pm6" | "pod6"
            | "t6" | "nim" | "nims" | "zig" | "v" | "vsh" => FileType::Code,
            "txt" | "rst" | "log" | "csv" | "tsv" | "json" | "xml" | "yaml"
            | "yml" | "toml" | "ini" | "conf" | "cfg" | "properties"
            | "env" | "sql" | "graphql" | "gql" => FileType::Text,
            "md" | "markdown" | "mkd" | "mdown" => FileType::Markdown,
            "org" => FileType::Org,
            "zip" | "rar" | "7z" | "tar" | "gz" | "bz2" | "xz" | "lz"
            | "lzma" | "zst" | "br" | "tgz" | "tbz" | "txz" | "tlz" | "cab"
            | "deb" | "rpm" | "dmg" | "pkg" | "msi" | "iso" | "img" => {
                FileType::Archive
            }
            "pdf" | "doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx"
            | "odt" | "ods" | "odp" | "rtf" | "epub" | "mobi" | "azw"
            | "azw3" | "tex" | "latex" => FileType::Document,
            "exe" | "dll" | "so" | "dylib" | "bin" | "app" | "elf" | "wasm"
            | "pyc" | "class" | "o" | "obj" | "lib" | "a" => {
                FileType::Executable
            }
            _ => FileType::Unknown,
        }
    }

    pub(crate) fn icon(&self) -> &'static str {
        match self {
            FileType::Directory => "📁",
            FileType::Image => "🖼️",
            FileType::Video => "🎬",
            FileType::Audio => "🎵",
            FileType::Code => "📄",
            FileType::Text => "📝",
            FileType::Markdown => "📜",
            FileType::Org => "📋",
            FileType::Archive => "📦",
            FileType::Document => "📑",
            FileType::Executable => "⚙️",
            FileType::Unknown => "📄",
        }
    }

    pub(crate) fn color(&self) -> &'static str {
        match self {
            FileType::Directory => "#2196f3",
            FileType::Image => "#e91e63",
            FileType::Video => "#f44336",
            FileType::Audio => "#9c27b0",
            FileType::Code => "#4caf50",
            FileType::Text => "#607d8b",
            FileType::Markdown => "#03a9f4",
            FileType::Org => "#00bcd4",
            FileType::Archive => "#ff9800",
            FileType::Document => "#3f51b5",
            FileType::Executable => "#795548",
            FileType::Unknown => "#9e9e9e",
        }
    }

    pub(crate) fn is_text(&self) -> bool {
        matches!(
            self,
            FileType::Code
                | FileType::Text
                | FileType::Markdown
                | FileType::Org
        )
    }
}

pub(crate) async fn serve_directory(
    rel_path: &str,
    full_path: &PathBuf,
    query: &RequestQuery,
    state: &ServerState,
) -> Response {
    let entries =
        match list_directory_entries(rel_path, full_path, query, state).await {
            Ok(e) => e,
            Err(e) => {
                warn!("Cannot read directory {}: {}", full_path.display(), e);
                return (StatusCode::FORBIDDEN, "Permission Denied")
                    .into_response();
            }
        };

    let html = generate_directory_html(rel_path, &entries, query, state);
    Html(html).into_response()
}

async fn list_directory_entries(
    rel_path: &str,
    full_path: &PathBuf,
    query: &RequestQuery,
    _state: &ServerState,
) -> std::io::Result<Vec<FileEntry>> {
    let mut entries = fs::read_dir(full_path).await?;
    let mut files = Vec::new();

    // Determine if we should show hidden files
    let show_hidden = if _state.allow_hidden {
        query.hidden.unwrap_or(true)
    } else {
        false
    };

    if !rel_path.is_empty() {
        files.push(FileEntry {
            name: "..".to_string(),
            path: "../".to_string(),
            is_dir: true,
            size: 0,
            modified_time: None,
            file_type: FileType::Directory,
        });
    }

    while let Some(entry) = entries.next_entry().await? {
        let name = entry.file_name().to_string_lossy().to_string();

        // Skip hidden files if not showing them
        if !show_hidden && name.starts_with('.') {
            continue;
        }

        let metadata = entry.metadata().await.ok();

        let is_dir = metadata.as_ref().map(|m| m.is_dir()).unwrap_or(false);
        let size = metadata.as_ref().map(|m| m.len()).unwrap_or(0);
        let modified_time = metadata.and_then(|m| m.modified().ok());

        let extension = Path::new(&name)
            .extension()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        let file_type = if is_dir {
            FileType::Directory
        } else {
            FileType::from_extension(&extension)
        };

        let path =
            format!("{}{}", encode_path(&name), if is_dir { "/" } else { "" });

        files.push(FileEntry {
            name,
            path,
            is_dir,
            size,
            modified_time,
            file_type,
        });
    }

    // Sort based on query parameter
    match query.sort {
        SortBy::Name => {
            files.sort_by(|a, b| match (a.is_dir, b.is_dir) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            });
        }
        SortBy::Size => {
            files.sort_by(|a, b| match (a.is_dir, b.is_dir) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => a.size.cmp(&b.size),
            });
        }
        SortBy::Time => {
            files.sort_by(|a, b| {
                match (a.is_dir, b.is_dir) {
                    (true, false) => std::cmp::Ordering::Less,
                    (false, true) => std::cmp::Ordering::Greater,
                    _ => b.modified_time.cmp(&a.modified_time), // Newest first
                }
            });
        }
        SortBy::Type => {
            files.sort_by(|a, b| match (a.is_dir, b.is_dir) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => {
                    let type_order = |ft: &FileType| match ft {
                        FileType::Directory => 0,
                        FileType::Code => 1,
                        FileType::Text => 2,
                        FileType::Markdown => 3,
                        FileType::Org => 4,
                        FileType::Image => 5,
                        FileType::Video => 6,
                        FileType::Audio => 7,
                        FileType::Document => 8,
                        FileType::Archive => 9,
                        FileType::Executable => 10,
                        FileType::Unknown => 11,
                    };
                    type_order(&a.file_type).cmp(&type_order(&b.file_type))
                }
            });
        }
    }

    Ok(files)
}

fn generate_breadcrumb(current_path: &str) -> String {
    if current_path.is_empty() {
        return "<a href=\"/\">/</a>".to_string();
    }

    let mut result = String::new();
    result.push_str("<a href=\"/\">/</a>");

    let parts: Vec<&str> =
        current_path.split('/').filter(|s| !s.is_empty()).collect();
    let mut cumulative_path = String::new();

    for (i, part) in parts.iter().enumerate() {
        cumulative_path.push('/');
        cumulative_path.push_str(part);

        if i == parts.len() - 1 {
            // Last part - current directory, not a link
            result.push_str(&format!("<span>{}</span>", html_escape(part)));
        } else {
            // Parent directory, make it a link
            result.push_str(&format!(
                "<a href=\"{}\">{}</a>/",
                encode_path(&cumulative_path),
                html_escape(part)
            ));
        }
    }

    result
}

fn generate_directory_html(
    current_path: &str,
    entries: &[FileEntry],
    query: &RequestQuery,
    state: &ServerState,
) -> String {
    use std::fmt::Write;

    let display_path = if current_path.is_empty() {
        "/".to_string()
    } else {
        format!("/{}/", current_path.trim_end_matches('/'))
    };

    let breadcrumb = generate_breadcrumb(current_path);

    // Determine current sort and hidden settings
    let current_sort = query.sort;
    let show_hidden = query.hidden.unwrap_or(true);

    // Build sort links (preserve other query params)
    let sort_link = |sort: SortBy| {
        let sort_name = match sort {
            SortBy::Name => "name",
            SortBy::Size => "size",
            SortBy::Time => "time",
            SortBy::Type => "type",
        };
        let hidden_param = if show_hidden {
            "&hidden=true"
        } else {
            "&hidden=false"
        };
        format!("?sort={}{}", sort_name, hidden_param)
    };

    // Build hidden toggle link
    let hidden_toggle_link = if state.allow_hidden {
        let sort_name = match current_sort {
            SortBy::Name => "name",
            SortBy::Size => "size",
            SortBy::Time => "time",
            SortBy::Type => "type",
        };
        let new_hidden = !show_hidden;
        format!("?sort={}&hidden={}", sort_name, new_hidden)
    } else {
        String::new()
    };

    let mut html = String::with_capacity(4096 + entries.len() * 256);

    let _ = write!(
        html,
        r##"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>Index of {}</title>
    <style>
        * {{ box-sizing: border-box; margin: 0; padding: 0; }}
        body {{
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif;
            line-height: 1.6;
            background: #0d1117;
            color: #c9d1d9;
            min-height: 100vh;
        }}
        .container {{ max-width: 1200px; margin: 0 auto; padding: 20px; }}
        header {{
            background: #161b22;
            border-bottom: 1px solid #30363d;
            padding: 20px;
            margin: -20px -20px 20px -20px;
        }}
        h1 {{ color: #f0f6fc; font-size: 1.5rem; font-weight: 600; margin-bottom: 12px; }}
        .breadcrumb {{ color: #8b949e; font-size: 0.95rem; }}
        .breadcrumb a {{ color: #58a6ff; text-decoration: none; }}
        .breadcrumb a:hover {{ text-decoration: underline; }}
        .breadcrumb span {{ color: #f0f6fc; font-weight: 500; }}
        .controls {{
            display: flex;
            gap: 20px;
            flex-wrap: wrap;
            align-items: center;
            margin-top: 16px;
            padding-top: 16px;
            border-top: 1px solid #30363d;
        }}
        .control-group {{ display: flex; align-items: center; gap: 8px; }}
        .control-label {{ color: #8b949e; font-size: 0.85rem; }}
        .btn-group {{ display: flex; gap: 4px; }}
        .btn {{
            padding: 4px 12px;
            border-radius: 6px;
            font-size: 0.85rem;
            text-decoration: none;
            border: 1px solid #30363d;
            background: #21262d;
            color: #c9d1d9;
            cursor: pointer;
        }}
        .btn:hover {{ background: #30363d; }}
        .btn.active {{ background: #1f6feb; border-color: #1f6feb; color: #fff; }}
        .btn.disabled {{ opacity: 0.5; cursor: not-allowed; }}
        .file-list {{
            background: #161b22;
            border: 1px solid #30363d;
            border-radius: 12px;
            overflow: hidden;
        }}
        .file-header {{
            display: grid;
            grid-template-columns: auto 1fr 100px 180px;
            gap: 16px;
            padding: 12px 20px;
            background: #21262d;
            border-bottom: 1px solid #30363d;
            font-size: 0.75rem;
            font-weight: 600;
            color: #8b949e;
            text-transform: uppercase;
            letter-spacing: 0.5px;
        }}
        .file-item {{
            display: grid;
            grid-template-columns: auto 1fr 100px 180px;
            gap: 16px;
            padding: 10px 20px;
            border-bottom: 1px solid #21262d;
            align-items: center;
        }}
        .file-item:hover {{ background: #1f242c; }}
        .file-item:last-child {{ border-bottom: none; }}
        .icon {{ font-size: 1.2rem; width: 24px; text-align: center; }}
        .filename {{ display: flex; align-items: center; gap: 10px; }}
        .filename a {{
            color: #c9d1d9;
            text-decoration: none;
            font-weight: 500;
            display: flex;
            align-items: center;
            gap: 8px;
        }}
        .filename a:hover {{ color: #58a6ff; }}
        .filename .dir {{ color: #58a6ff; }}
        .file-type-badge {{
            font-size: 0.65rem;
            padding: 2px 6px;
            border-radius: 4px;
            background: #30363d;
            color: #8b949e;
            text-transform: uppercase;
            font-weight: 600;
        }}
        .size {{ text-align: right; color: #8b949e; font-family: monospace; font-size: 0.9rem; }}
        .time {{ color: #8b949e; font-size: 0.85rem; }}
        footer {{
            margin-top: 40px;
            padding: 20px;
            text-align: center;
            color: #484f58;
            font-size: 0.85rem;
        }}
    </style>
</head>
<body>
    <div class="container">
        <header>
            <h1>📁 Index of <span class="breadcrumb">{}</span></h1>
            <div class="breadcrumb">{}</div>
            <div class="controls">
                <div class="control-group">
                    <span class="control-label">Sort by:</span>
                    <div class="btn-group">
                        <a href="{}" class="btn {}">Name</a>
                        <a href="{}" class="btn {}">Size</a>
                        <a href="{}" class="btn {}">Time</a>
                        <a href="{}" class="btn {}">Type</a>
                    </div>
                </div>"##,
        html_escape(&display_path),
        html_escape(&display_path),
        breadcrumb,
        sort_link(SortBy::Name),
        if matches!(current_sort, SortBy::Name) {
            "active"
        } else {
            ""
        },
        sort_link(SortBy::Size),
        if matches!(current_sort, SortBy::Size) {
            "active"
        } else {
            ""
        },
        sort_link(SortBy::Time),
        if matches!(current_sort, SortBy::Time) {
            "active"
        } else {
            ""
        },
        sort_link(SortBy::Type),
        if matches!(current_sort, SortBy::Type) {
            "active"
        } else {
            ""
        }
    );

    // Add hidden files toggle if allowed
    if state.allow_hidden {
        let hidden_btn_class = if show_hidden { "active" } else { "" };
        let hidden_text = if show_hidden {
            "Hide hidden"
        } else {
            "Show hidden"
        };
        let _ = write!(
            html,
            r#"
                <div class="control-group">
                    <span class="control-label">Hidden files:</span>
                    <a href="{}" class="btn {}">{}</a>
                </div>"#,
            hidden_toggle_link, hidden_btn_class, hidden_text
        );
    } else {
        let _ = write!(
            html,
            r#"
                <div class="control-group">
                    <span class="control-label">Hidden files:</span>
                    <span class="btn disabled" title="Disabled by server">Disabled</span>
                </div>"#
        );
    }

    let _ = write!(
        html,
        r#"
            </div>
        </header>
        <div class="file-list">
            <div class="file-header">
                <span></span>
                <span>Name</span>
                <span class="size">Size</span>
                <span class="time">Modified</span>
            </div>"#
    );

    for entry in entries {
        let size_display = if entry.is_dir {
            "-".to_string()
        } else {
            format_size(entry.size)
        };
        let time_display = entry
            .modified_time
            .map(format_http_date)
            .unwrap_or_else(|| "-".to_string());

        let is_dir_class = if entry.is_dir { "dir" } else { "" };
        let file_type_name = format!("{:?}", entry.file_type);

        let _ = write!(
            html,
            r#"
            <div class="file-item">
                <span class="icon">{}</span>
                <div class="filename">
                    <a href="{}" class="{}">
                        {}
                        {}
                    </a>
                </div>
                <span class="size">{}</span>
                <span class="time">{}</span>
            </div>"#,
            entry.file_type.icon(),
            entry.path,
            is_dir_class,
            html_escape(&entry.name),
            if entry.is_dir {
                String::new()
            } else {
                format!(
                    "<span class='file-type-badge' style='background: {}; color: #fff;'>{}</span>",
                    entry.file_type.color(),
                    file_type_name
                )
            },
            size_display,
            time_display
        );
    }

    let _ = write!(
        html,
        r#"
        </div>
        <footer>
            <p>D HTTP Server · {} items</p>
        </footer>
    </div>
</body>
</html>"#,
        entries.len()
    );

    html
}
