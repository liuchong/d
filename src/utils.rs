use percent_encoding::{percent_decode_str, percent_encode, AsciiSet, CONTROLS};
use std::path::Path;

/// Path percent encode set: https://url.spec.whatwg.org/#path-percent-encode-set
const PATH_ENCODE_SET: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'<')
    .add(b'>')
    .add(b'?')
    .add(b'`')
    .add(b'{')
    .add(b'}');

/// Decode percent-encoded string
pub fn decode_path(s: &str) -> Result<String, std::str::Utf8Error> {
    percent_decode_str(s).decode_utf8().map(|s| s.to_string())
}

/// Percent-encode path component for URL generation
pub fn encode_path(s: &str) -> impl std::fmt::Display + use<'_> {
    percent_encode(s.as_bytes(), PATH_ENCODE_SET)
}

/// Guess MIME type from filename
pub fn guess_mime_type(filename: &str) -> String {
    let ext = Path::new(filename)
        .extension()
        .unwrap_or_default()
        .to_str()
        .unwrap_or_default();
    
    mime_guess::from_ext(ext)
        .first()
        .map(|m| m.to_string())
        .unwrap_or_else(|| "application/octet-stream".to_string())
}

/// Format file size in human readable format
pub fn format_size(size: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    if size == 0 {
        return "0 B".to_string();
    }
    let exp = (size as f64).log(1024.0).min(UNITS.len() as f64 - 1.0) as usize;
    let size = size as f64 / 1024f64.powi(exp as i32);
    if exp == 0 {
        format!("{} {}", size as u64, UNITS[exp])
    } else {
        format!("{:.2} {}", size, UNITS[exp])
    }
}

/// Format system time to HTTP date format
pub fn format_http_date(modified: std::time::SystemTime) -> String {
    let datetime: time::OffsetDateTime = modified.into();
    datetime.format(&time::format_description::well_known::Rfc2822)
        .unwrap_or_default()
}
