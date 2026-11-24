//! Static file streaming, Range (206/416), ETag/IMS 304, HEAD, and RFC 5987 download.

use crate::httpdate::{format_http_date, parse_http_date};
use crate::mime::mime_for_path;
use files_api::shell::percent_encode_segment;
use http_api::{AsyncByteStream, HttpResponse, RequestMeta, StatusCode};
use std::fs::Metadata;
use std::io::{self, SeekFrom};
use std::path::Path;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::UNIX_EPOCH;
use tokio::fs::File;
use tokio::io::{AsyncRead, AsyncSeekExt, ReadBuf};

const STREAM_CHUNK_BYTES: usize = 64 * 1024;

/// Inclusive `[start, end]` byte range inside a static file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ByteRange {
    pub start: u64,
    pub end: u64,
}

/// Outcome of evaluating an HTTP `Range` request header against `file_size`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParsedRange {
    Single(ByteRange),
    Unsatisfiable,
}

struct FileChunkStream {
    file: File,
    buf: Vec<u8>,
    remaining: u64,
}

impl FileChunkStream {
    fn new(file: File, total_len: u64) -> Self {
        Self {
            file,
            buf: vec![0u8; STREAM_CHUNK_BYTES],
            remaining: total_len,
        }
    }
}

impl AsyncByteStream for FileChunkStream {
    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<io::Result<Vec<u8>>>> {
        if self.remaining == 0 {
            return Poll::Ready(None);
        }
        let max_read = (self.remaining as usize).min(self.buf.len());
        let me = &mut *self;
        let mut read_buf = ReadBuf::new(&mut me.buf[..max_read]);
        match Pin::new(&mut me.file).poll_read(cx, &mut read_buf) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Err(e)) => Poll::Ready(Some(Err(e))),
            Poll::Ready(Ok(())) => {
                let n = read_buf.filled().len();
                if n == 0 {
                    me.remaining = 0;
                    Poll::Ready(None)
                } else {
                    me.remaining = me.remaining.saturating_sub(n as u64);
                    Poll::Ready(Some(Ok(read_buf.filled().to_vec())))
                }
            }
        }
    }
}

/// Parse an HTTP `Range: bytes=...` header value against `file_size`.
pub fn parse_range(range_header: &str, file_size: u64) -> Option<ParsedRange> {
    let spec = range_header.trim().strip_prefix("bytes=")?;
    if spec.contains(',') {
        return None;
    }
    if file_size == 0 {
        return Some(ParsedRange::Unsatisfiable);
    }

    let (start_str, end_str) = spec.split_once('-')?;

    if start_str.is_empty() {
        let suffix_len: u64 = end_str.trim().parse().ok()?;
        if suffix_len == 0 {
            return Some(ParsedRange::Unsatisfiable);
        }
        let start = file_size.saturating_sub(suffix_len);
        let end = file_size - 1;
        return Some(ParsedRange::Single(ByteRange { start, end }));
    }

    let start: u64 = start_str.trim().parse().ok()?;
    if start >= file_size {
        return Some(ParsedRange::Unsatisfiable);
    }

    let end = if end_str.is_empty() {
        file_size - 1
    } else {
        let parsed: u64 = end_str.trim().parse().ok()?;
        if parsed < start {
            return Some(ParsedRange::Unsatisfiable);
        }
        parsed.min(file_size - 1)
    };

    Some(ParsedRange::Single(ByteRange { start, end }))
}

/// Generate a strong ETag derived from modification timestamp seconds and size.
pub fn generate_etag(metadata: &Metadata) -> String {
    let mtime = metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs());
    format!("\"{mtime}-{}\"", metadata.len())
}

/// Evaluate `If-None-Match` and `If-Modified-Since` preconditions.
pub fn is_not_modified(
    req: &RequestMeta,
    etag: &str,
    modified: Option<std::time::SystemTime>,
) -> bool {
    if let Some(inm) = req.headers.get("if-none-match") {
        let matched = inm.split(',').any(|candidate| {
            let t = candidate.trim();
            t == "*" || t == etag || t.strip_prefix("W/") == Some(etag)
        });
        return matched;
    }

    if let Some(ims) = req.headers.get("if-modified-since")
        && let Some(ims_time) = parse_http_date(ims)
        && let Some(mod_time) = modified
    {
        let mod_secs = mod_time
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let ims_secs = ims_time
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        return mod_secs <= ims_secs;
    }

    false
}

/// Format an attachment `Content-Disposition` with ASCII fallback and RFC 5987 UTF-8 name.
pub fn content_disposition(path: &str) -> String {
    let name = path.rsplit('/').next().unwrap_or(path);
    let ascii_safe: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ' ') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let encoded_utf8 = percent_encode_segment(name);
    format!(
        "attachment; filename=\"{ascii_safe}\"; filename*=UTF-8''{encoded_utf8}"
    )
}

/// Serve a regular file supporting `HEAD`, `ETag`/`304`, and single `Range` (`206`/`416`).
pub async fn serve_file(
    file_path: &Path,
    metadata: &Metadata,
    req: &RequestMeta,
    is_head: bool,
) -> HttpResponse {
    let file_size = metadata.len();
    let content_type = mime_for_path(file_path);
    let etag = generate_etag(metadata);
    let modified = metadata.modified().ok();
    let last_modified = modified.map(format_http_date);

    if is_not_modified(req, &etag, modified) {
        let mut resp = HttpResponse::empty(StatusCode::NOT_MODIFIED)
            .with_header("ETag", etag);
        if let Some(lm) = last_modified {
            resp.headers.insert("Last-Modified", lm);
        }
        return resp;
    }

    let range_result = req
        .headers
        .get("range")
        .and_then(|r| parse_range(r, file_size));

    if let Some(ParsedRange::Unsatisfiable) = range_result {
        let mut resp = HttpResponse::empty(StatusCode::RANGE_NOT_SATISFIABLE)
            .with_header("Content-Range", format!("bytes */{file_size}"))
            .with_header("ETag", etag);
        if let Some(lm) = last_modified {
            resp.headers.insert("Last-Modified", lm);
        }
        return resp;
    }

    let (status, start, len, content_range) =
        if let Some(ParsedRange::Single(br)) = range_result {
            let len = br.end - br.start + 1;
            (
                StatusCode::PARTIAL_CONTENT,
                br.start,
                len,
                Some(format!("bytes {}-{}/{file_size}", br.start, br.end)),
            )
        } else {
            (StatusCode::OK, 0, file_size, None)
        };

    let mut resp = if is_head {
        let mut r = HttpResponse::empty(status)
            .with_header("Content-Type", content_type)
            .with_header("Content-Length", len.to_string())
            .with_header("Accept-Ranges", "bytes")
            .with_header("ETag", etag);
        if let Some(lm) = last_modified {
            r.headers.insert("Last-Modified", lm);
        }
        r
    } else {
        let mut file = match File::open(file_path).await {
            Ok(f) => f,
            Err(_) => {
                return HttpResponse::text(StatusCode::NOT_FOUND, "Not Found");
            }
        };
        if start > 0 && file.seek(SeekFrom::Start(start)).await.is_err() {
            return HttpResponse::text(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Seek Failed",
            );
        }
        let stream = Box::new(FileChunkStream::new(file, len));
        let mut r = HttpResponse::stream(
            status,
            content_type,
            stream,
            Some(len),
            false,
        )
        .with_header("Accept-Ranges", "bytes")
        .with_header("ETag", etag);
        if let Some(lm) = last_modified {
            r.headers.insert("Last-Modified", lm);
        }
        r
    };

    if let Some(cr) = content_range {
        resp.headers.insert("Content-Range", cr);
    }

    resp
}

/// Serve raw uncompressed/unwrapped file bytes (`?view=raw`).
pub async fn serve_raw_file(
    file_path: &Path,
    metadata: &Metadata,
    is_head: bool,
) -> HttpResponse {
    let file_size = metadata.len();
    let content_type = mime_for_path(file_path);

    if is_head {
        return HttpResponse::empty(StatusCode::OK)
            .with_header("Content-Type", content_type)
            .with_header("Content-Length", file_size.to_string());
    }

    let file = match File::open(file_path).await {
        Ok(f) => f,
        Err(_) => {
            return HttpResponse::text(StatusCode::NOT_FOUND, "Not Found");
        }
    };
    HttpResponse::stream(
        StatusCode::OK,
        content_type,
        Box::new(FileChunkStream::new(file, file_size)),
        Some(file_size),
        false,
    )
}

/// Serve file forced as an RFC 5987 attachment download (`?view=download`).
pub async fn serve_download(
    file_path: &Path,
    rel_path: &str,
    metadata: &Metadata,
    is_head: bool,
) -> HttpResponse {
    let file_size = metadata.len();
    let disposition = content_disposition(rel_path);

    if is_head {
        return HttpResponse::empty(StatusCode::OK)
            .with_header("Content-Type", "application/octet-stream")
            .with_header("Content-Length", file_size.to_string())
            .with_header("Content-Disposition", disposition);
    }

    let file = match File::open(file_path).await {
        Ok(f) => f,
        Err(_) => {
            return HttpResponse::text(StatusCode::NOT_FOUND, "Not Found");
        }
    };
    HttpResponse::stream(
        StatusCode::OK,
        "application/octet-stream",
        Box::new(FileChunkStream::new(file, file_size)),
        Some(file_size),
        false,
    )
    .with_header("Content-Disposition", disposition)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_parsing_covers_all_cases() {
        assert_eq!(
            parse_range("bytes=5-9", 16),
            Some(ParsedRange::Single(ByteRange { start: 5, end: 9 }))
        );
        assert_eq!(
            parse_range("bytes=-4", 16),
            Some(ParsedRange::Single(ByteRange { start: 12, end: 15 }))
        );
        assert_eq!(
            parse_range("bytes=-100", 16),
            Some(ParsedRange::Single(ByteRange { start: 0, end: 15 }))
        );
        assert_eq!(
            parse_range("bytes=-0", 16),
            Some(ParsedRange::Unsatisfiable)
        );
        assert_eq!(
            parse_range("bytes=5-", 16),
            Some(ParsedRange::Single(ByteRange { start: 5, end: 15 }))
        );
        assert_eq!(
            parse_range("bytes=5-999", 16),
            Some(ParsedRange::Single(ByteRange { start: 5, end: 15 }))
        );
        assert_eq!(
            parse_range("bytes=16-20", 16),
            Some(ParsedRange::Unsatisfiable)
        );
        assert_eq!(
            parse_range("bytes=9-5", 16),
            Some(ParsedRange::Unsatisfiable)
        );
        assert_eq!(
            parse_range("bytes=0-0", 0),
            Some(ParsedRange::Unsatisfiable)
        );
        assert_eq!(parse_range("bytes=0-1,2-3", 16), None);
        assert_eq!(parse_range("items=0-5", 16), None);
    }

    #[test]
    fn disposition_formats_ascii_and_utf8() {
        let ascii = content_disposition("/dir/code.rs");
        assert!(ascii.contains(
            "attachment; filename=\"code.rs\"; filename*=UTF-8''code.rs"
        ));

        let utf8 = content_disposition("/中文.txt");
        assert!(utf8.contains("filename*=UTF-8''%E4%B8%AD%E6%96%87.txt"));
    }
}
