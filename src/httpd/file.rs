//! File serving: full/partial content, conditional requests and downloads.

use crate::utils::guess_mime_type;
use axum::{
    body::Body,
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, percent_encode};
use std::path::Path;
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_util::io::ReaderStream;
use tracing::error;

/// Outcome of parsing a `Range` header against a known file size.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ByteRange {
    /// Inclusive byte offsets to serve.
    Satisfiable(u64, u64),
    /// Well-formed but cannot be satisfied for this file (respond 416).
    Unsatisfiable,
}

/// Parse a single `Range` header value (`bytes=start-end`, `bytes=start-`,
/// `bytes=-suffix`). Returns `None` when the header is malformed and must be
/// ignored (multi-range requests with `,` are intentionally unsupported and
/// ignored, falling back to a full 200 response).
pub(crate) fn parse_range(range: &str, file_size: u64) -> Option<ByteRange> {
    let range = range.strip_prefix("bytes=")?;

    // Multi-range (`bytes=0-1,3-4`) is not supported: ignore the header.
    if range.contains(',') {
        return None;
    }

    let (first, second) = range.split_once('-')?;

    if file_size == 0 {
        return Some(ByteRange::Unsatisfiable);
    }

    if first.is_empty() {
        // Suffix range: last N bytes.
        let n: u64 = second.parse().ok()?;
        if n == 0 {
            return Some(ByteRange::Unsatisfiable);
        }
        if n >= file_size {
            return Some(ByteRange::Satisfiable(0, file_size - 1));
        }
        return Some(ByteRange::Satisfiable(file_size - n, file_size - 1));
    }

    let start: u64 = first.parse().ok()?;
    if start >= file_size {
        return Some(ByteRange::Unsatisfiable);
    }
    let end = if second.is_empty() {
        file_size - 1
    } else {
        // Clamp an over-large end to the last byte, per RFC 9110.
        second.parse::<u64>().ok()?.min(file_size - 1)
    };
    if start > end {
        return Some(ByteRange::Unsatisfiable);
    }

    Some(ByteRange::Satisfiable(start, end))
}

pub(crate) fn generate_etag(metadata: &std::fs::Metadata) -> String {
    let modified = metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("\"{:x}-{:x}\"", modified, metadata.len())
}

/// Check whether the request's conditional headers match the current
/// representation, meaning a `304 Not Modified` can be sent. Per RFC 9110,
/// `If-None-Match` takes precedence over `If-Modified-Since`.
fn is_not_modified(
    req_headers: &HeaderMap,
    etag: &str,
    metadata: &std::fs::Metadata,
) -> bool {
    if let Some(value) = req_headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
    {
        // Weak comparison: ignore the W/ prefix; support lists and `*`.
        let current = etag.trim_start_matches("W/");
        return value.split(',').any(|candidate| {
            let candidate = candidate.trim();
            candidate == "*" || candidate.trim_start_matches("W/") == current
        });
    }

    if let Some(value) = req_headers
        .get(header::IF_MODIFIED_SINCE)
        .and_then(|v| v.to_str().ok())
        && let (Ok(since), Ok(modified)) =
            (httpdate::parse_http_date(value), metadata.modified())
    {
        // Compare at second granularity: timestamps from the filesystem
        // may carry sub-second precision the header cannot express.
        let since_secs = since
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let modified_secs = modified
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        return modified_secs <= since_secs;
    }

    false
}

/// IMF-fixdate (`Sun, 06 Nov 1994 08:49:37 GMT`), the only date format a
/// server may generate in `Last-Modified` per RFC 9110 section 5.6.7.
const IMF_FIXDATE: &[time::format_description::FormatItem<'_>] = time::macros::format_description!(
    "[weekday repr:short], [day padding:zero] [month repr:short] [year] [hour]:[minute]:[second] GMT"
);

/// Insert `Last-Modified` into `headers` when the file's mtime is known.
fn insert_last_modified(headers: &mut HeaderMap, metadata: &std::fs::Metadata) {
    if let Ok(modified) = metadata.modified()
        && let Ok(time_str) =
            time::OffsetDateTime::from(modified).format(IMF_FIXDATE)
    {
        headers.insert(
            header::LAST_MODIFIED,
            time_str
                .parse()
                .expect("IMF-fixdate is a valid header value"),
        );
    }
}

/// Build the base response headers shared by 200/206/304 responses.
fn base_headers(
    path: &Path,
    metadata: &std::fs::Metadata,
    etag: &str,
) -> HeaderMap {
    let mut headers = HeaderMap::new();

    let mime = guess_mime_type(
        path.file_name().and_then(|n| n.to_str()).unwrap_or(""),
    );
    headers.insert(
        header::CONTENT_TYPE,
        mime.parse()
            .expect("mime_guess output is a valid MIME type"),
    );
    headers.insert(
        header::ACCEPT_RANGES,
        "bytes".parse().expect("'bytes' is a valid header value"),
    );
    headers.insert(
        header::CACHE_CONTROL,
        "public, max-age=3600"
            .parse()
            .expect("static value is valid"),
    );
    headers
        .insert(header::ETAG, etag.parse().expect("generated ETag is valid"));
    insert_last_modified(&mut headers, metadata);

    headers
}

/// Open `path` or log and produce a 500 response.
async fn open_file(path: &Path) -> Result<fs::File, Response> {
    fs::File::open(path).await.map_err(|e| {
        error!("Failed to open file {}: {}", path.display(), e);
        (StatusCode::INTERNAL_SERVER_ERROR, "Internal Server Error")
            .into_response()
    })
}

pub(crate) async fn serve_file(
    path: &Path,
    metadata: &std::fs::Metadata,
    req_headers: &HeaderMap,
    is_head: bool,
) -> Response {
    let file_size = metadata.len();
    let etag = generate_etag(metadata);
    let headers = base_headers(path, metadata, &etag);

    // Conditional request: the client already has a fresh copy.
    if is_not_modified(req_headers, &etag, metadata) {
        return (StatusCode::NOT_MODIFIED, headers).into_response();
    }

    if let Some(range_value) =
        req_headers.get(header::RANGE).and_then(|v| v.to_str().ok())
    {
        match parse_range(range_value, file_size) {
            Some(ByteRange::Unsatisfiable) => {
                let mut headers = HeaderMap::new();
                headers.insert(
                    header::CONTENT_RANGE,
                    format!("bytes */{}", file_size)
                        .parse()
                        .expect("generated Content-Range is valid"),
                );
                return (StatusCode::RANGE_NOT_SATISFIABLE, headers)
                    .into_response();
            }
            Some(ByteRange::Satisfiable(start, end)) => {
                let mut headers = headers;
                let content_length = end - start + 1;
                headers.insert(
                    header::CONTENT_LENGTH,
                    content_length
                        .to_string()
                        .parse()
                        .expect("u64 is a valid header value"),
                );
                headers.insert(
                    header::CONTENT_RANGE,
                    format!("bytes {}-{}/{}", start, end, file_size)
                        .parse()
                        .expect("generated Content-Range is valid"),
                );

                if is_head {
                    return (StatusCode::PARTIAL_CONTENT, headers)
                        .into_response();
                }

                let mut file = match open_file(path).await {
                    Ok(f) => f,
                    Err(resp) => return resp,
                };
                if let Err(e) = file.seek(std::io::SeekFrom::Start(start)).await
                {
                    error!("Failed to seek file {}: {}", path.display(), e);
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "Internal Server Error",
                    )
                        .into_response();
                }

                let stream = ReaderStream::new(file.take(content_length));
                let body = Body::from_stream(stream);

                return (StatusCode::PARTIAL_CONTENT, headers, body)
                    .into_response();
            }
            None => {
                // Malformed Range header: ignore it, serve the full file.
            }
        }
    }

    let mut headers = headers;
    headers.insert(
        header::CONTENT_LENGTH,
        file_size
            .to_string()
            .parse()
            .expect("u64 is a valid header value"),
    );

    if is_head {
        return (StatusCode::OK, headers).into_response();
    }

    let file = match open_file(path).await {
        Ok(f) => f,
        Err(resp) => return resp,
    };
    let stream = ReaderStream::new(file);
    let body = Body::from_stream(stream);

    (StatusCode::OK, headers, body).into_response()
}

pub(crate) async fn serve_raw_file(
    path: &Path,
    metadata: &std::fs::Metadata,
) -> Response {
    let file = match open_file(path).await {
        Ok(f) => f,
        Err(resp) => return resp,
    };

    let mut headers = HeaderMap::new();
    let mime = guess_mime_type(
        path.file_name().and_then(|n| n.to_str()).unwrap_or(""),
    );
    headers.insert(
        header::CONTENT_TYPE,
        mime.parse()
            .expect("mime_guess output is a valid MIME type"),
    );
    headers.insert(
        header::CONTENT_LENGTH,
        metadata
            .len()
            .to_string()
            .parse()
            .expect("u64 is a valid header value"),
    );

    let stream = ReaderStream::new(file);
    let body = Body::from_stream(stream);

    (StatusCode::OK, headers, body).into_response()
}

/// RFC 5987 attr-char set: characters allowed unencoded in `filename*`.
const ATTR_CHAR_ENCODE_SET: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'!')
    .remove(b'#')
    .remove(b'$')
    .remove(b'&')
    .remove(b'+')
    .remove(b'-')
    .remove(b'.')
    .remove(b'^')
    .remove(b'_')
    .remove(b'`')
    .remove(b'|')
    .remove(b'~');

/// Build a `Content-Disposition` header value per RFC 5987/RFC 6266:
/// an ASCII-only `filename` fallback plus a percent-encoded `filename*`.
pub(crate) fn content_disposition(filename: &str) -> String {
    let name = Path::new(filename)
        .file_name()
        .map(|n| n.to_string_lossy())
        .unwrap_or_default();

    let mut fallback = String::with_capacity(name.len());
    for c in name.chars() {
        match c {
            '"' | '\\' => fallback.push('_'),
            c if c.is_ascii() => fallback.push(c),
            _ => fallback.push('_'),
        }
    }

    let encoded =
        percent_encode(name.as_bytes(), ATTR_CHAR_ENCODE_SET).to_string();
    format!(
        "attachment; filename=\"{}\"; filename*=UTF-8''{}",
        fallback, encoded
    )
}

pub(crate) async fn serve_download(
    path: &Path,
    filename: &str,
    metadata: &std::fs::Metadata,
) -> Response {
    let file = match open_file(path).await {
        Ok(f) => f,
        Err(resp) => return resp,
    };

    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_DISPOSITION,
        content_disposition(filename)
            .parse()
            .expect("generated Content-Disposition is valid"),
    );
    headers.insert(
        header::CONTENT_LENGTH,
        metadata
            .len()
            .to_string()
            .parse()
            .expect("u64 is a valid header value"),
    );

    let stream = ReaderStream::new(file);
    let body = Body::from_stream(stream);

    (StatusCode::OK, headers, body).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_full_span() {
        assert_eq!(
            parse_range("bytes=0-99", 100),
            Some(ByteRange::Satisfiable(0, 99))
        );
    }

    #[test]
    fn range_middle_slice() {
        assert_eq!(
            parse_range("bytes=5-9", 100),
            Some(ByteRange::Satisfiable(5, 9))
        );
    }

    #[test]
    fn range_open_end() {
        assert_eq!(
            parse_range("bytes=10-", 100),
            Some(ByteRange::Satisfiable(10, 99))
        );
    }

    #[test]
    fn range_end_clamped_to_file_size() {
        assert_eq!(
            parse_range("bytes=90-999", 100),
            Some(ByteRange::Satisfiable(90, 99))
        );
    }

    #[test]
    fn range_suffix() {
        assert_eq!(
            parse_range("bytes=-10", 100),
            Some(ByteRange::Satisfiable(90, 99))
        );
    }

    #[test]
    fn range_suffix_larger_than_file() {
        assert_eq!(
            parse_range("bytes=-500", 100),
            Some(ByteRange::Satisfiable(0, 99))
        );
    }

    #[test]
    fn range_suffix_zero_is_unsatisfiable() {
        assert_eq!(
            parse_range("bytes=-0", 100),
            Some(ByteRange::Unsatisfiable)
        );
    }

    #[test]
    fn range_start_beyond_size_is_unsatisfiable() {
        assert_eq!(
            parse_range("bytes=100-", 100),
            Some(ByteRange::Unsatisfiable)
        );
        assert_eq!(
            parse_range("bytes=999999-", 100),
            Some(ByteRange::Unsatisfiable)
        );
    }

    #[test]
    fn range_inverted_is_unsatisfiable() {
        assert_eq!(
            parse_range("bytes=9-5", 100),
            Some(ByteRange::Unsatisfiable)
        );
    }

    #[test]
    fn range_on_empty_file_is_unsatisfiable() {
        assert_eq!(parse_range("bytes=0-0", 0), Some(ByteRange::Unsatisfiable));
        assert_eq!(parse_range("bytes=-1", 0), Some(ByteRange::Unsatisfiable));
    }

    #[test]
    fn range_multi_is_ignored() {
        assert_eq!(parse_range("bytes=0-1,3-4", 100), None);
    }

    #[test]
    fn range_malformed_is_ignored() {
        assert_eq!(parse_range("items=0-1", 100), None);
        assert_eq!(parse_range("bytes=", 100), None);
        assert_eq!(parse_range("bytes=a-b", 100), None);
        assert_eq!(parse_range("bytes=1-2-3", 100), None);
    }

    #[test]
    fn etag_contains_size() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.bin");
        std::fs::write(&path, [0u8; 42]).unwrap();
        let metadata = std::fs::metadata(&path).unwrap();

        let etag = generate_etag(&metadata);
        assert!(etag.starts_with('"') && etag.ends_with('"'));
        assert!(etag.contains("-2a")); // 42 in hex
    }

    #[test]
    fn disposition_ascii_name() {
        assert_eq!(
            content_disposition("report.pdf"),
            "attachment; filename=\"report.pdf\"; filename*=UTF-8''report.pdf"
        );
    }

    #[test]
    fn disposition_strips_directory() {
        assert_eq!(
            content_disposition("docs/report.pdf"),
            "attachment; filename=\"report.pdf\"; filename*=UTF-8''report.pdf"
        );
    }

    #[test]
    fn disposition_non_ascii_uses_rfc5987() {
        let value = content_disposition("报告.pdf");
        assert!(value.contains("filename=\"__.pdf\""));
        assert!(
            value.contains("filename*=UTF-8''%E6%8A%A5%E5%91%8A.pdf"),
            "got: {}",
            value
        );
        // Must be a valid header value (ASCII only).
        assert!(value.is_ascii());
    }

    #[test]
    fn disposition_quotes_and_backslashes_sanitized() {
        let value = content_disposition("a\"b\\c.txt");
        assert!(value.contains("filename=\"a_b_c.txt\""));
    }
}
