use std::path::Path;

pub fn decode_path(path: &str) -> Result<String, std::str::Utf8Error> {
    percent_encoding::percent_decode_str(path)
        .decode_utf8()
        .map(|s| s.to_string())
}

pub fn encode_path(path: &str) -> String {
    percent_encoding::utf8_percent_encode(path, percent_encoding::NON_ALPHANUMERIC)
        .to_string()
}

pub fn format_http_date(time: std::time::SystemTime) -> String {
    time::OffsetDateTime::from(time)
        .format(&time::format_description::well_known::Rfc2822)
        .unwrap_or_default()
}

pub fn format_size(size: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    if size == 0 {
        return "0 B".to_string();
    }
    let exp = (size as f64).log(1024.0).min(UNITS.len() as f64 - 1.0) as usize;
    let value = size as f64 / 1024f64.powi(exp as i32);
    if exp == 0 {
        format!("{} {}", size, UNITS[0])
    } else {
        format!("{:.2} {}", value, UNITS[exp])
    }
}

pub fn guess_mime_type(filename: &str) -> &'static str {
    let ext = Path::new(filename)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    match ext.as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" => "application/javascript; charset=utf-8",
        "json" => "application/json",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "ico" => "image/x-icon",
        "webp" => "image/webp",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mp3" => "audio/mpeg",
        "ogg" => "audio/ogg",
        "wav" => "audio/wav",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        "tar" => "application/x-tar",
        "gz" => "application/gzip",
        "txt" | "md" | "rs" | "py" | "ts" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

pub fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
