//! Pure-std file extension to MIME type inference table.

use std::path::Path;

/// Infer the `Content-Type` header value for a file path without external crates.
pub fn mime_for_path(path: &Path) -> &'static str {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    match ext.as_deref() {
        Some("html" | "htm") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js" | "mjs" | "cjs") => "text/javascript; charset=utf-8",
        Some("json") => "application/json; charset=utf-8",
        Some("xml") => "application/xml; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("avif") => "image/avif",
        Some("bmp") => "image/bmp",
        Some("ico") => "image/x-icon",
        Some("tiff" | "tif") => "image/tiff",
        Some("mp4" | "m4v") => "video/mp4",
        Some("webm") => "video/webm",
        Some("mkv") => "video/x-matroska",
        Some("mov") => "video/quicktime",
        Some("avi") => "video/x-msvideo",
        Some("ogv") => "video/ogg",
        Some("mp3") => "audio/mpeg",
        Some("wav") => "audio/wav",
        Some("ogg") => "audio/ogg",
        Some("flac") => "audio/flac",
        Some("aac") => "audio/aac",
        Some("m4a") => "audio/mp4",
        Some("opus") => "audio/opus",
        Some("pdf") => "application/pdf",
        Some("wasm") => "application/wasm",
        Some("woff") => "font/woff",
        Some("woff2") => "font/woff2",
        Some("ttf") => "font/ttf",
        Some("otf") => "font/otf",
        Some("zip") => "application/zip",
        Some("gz") => "application/gzip",
        Some("tar") => "application/x-tar",
        Some("md" | "markdown" | "mkd" | "mdown") => {
            "text/markdown; charset=utf-8"
        }
        Some(
            "txt" | "log" | "csv" | "tsv" | "yaml" | "yml" | "toml" | "ini"
            | "conf" | "cfg" | "env" | "org" | "rst" | "rs" | "py" | "ts"
            | "tsx" | "jsx" | "go" | "c" | "cpp" | "cc" | "h" | "hpp" | "java"
            | "rb" | "sh" | "bash" | "zsh" | "sql" | "graphql" | "gql",
        ) => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_in_mime_covers_web_media_and_code() {
        assert_eq!(
            mime_for_path(Path::new("index.html")),
            "text/html; charset=utf-8"
        );
        assert_eq!(mime_for_path(Path::new("a.png")), "image/png");
        assert_eq!(mime_for_path(Path::new("a.svg")), "image/svg+xml");
        assert_eq!(mime_for_path(Path::new("a.mp4")), "video/mp4");
        assert_eq!(mime_for_path(Path::new("a.mp3")), "audio/mpeg");
        assert_eq!(
            mime_for_path(Path::new("a.unknown")),
            "application/octet-stream"
        );
    }
}
