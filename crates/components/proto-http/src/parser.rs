//! Self-built HTTP/1.1 incremental wire parser with strict resource limits.

use crate::HttpConfig;
use http_api::{
    BodyKind, Headers, HttpRequest, Method, StatusCode, Version,
    split_uri_target,
};
use net_api::{BoxStreamIo, DrainSignal};
use std::future::poll_fn;
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

/// Wire-level parse failure mapped cleanly onto an HTTP error status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ParseRejection {
    BadRequest,
    PayloadTooLarge,
    UriTooLong,
    HeaderFieldsTooLarge,
    HttpVersionNotSupported,
}

impl ParseRejection {
    pub(crate) fn status_and_message(
        self,
    ) -> Option<(StatusCode, &'static str)> {
        match self {
            Self::BadRequest => Some((StatusCode::BAD_REQUEST, "Bad Request")),
            Self::PayloadTooLarge => {
                Some((StatusCode::PAYLOAD_TOO_LARGE, "Payload Too Large"))
            }
            Self::UriTooLong => {
                Some((StatusCode::URI_TOO_LONG, "URI Too Long"))
            }
            Self::HeaderFieldsTooLarge => Some((
                StatusCode::HEADER_FIELDS_TOO_LARGE,
                "Request Header Fields Too Large",
            )),
            Self::HttpVersionNotSupported => Some((
                StatusCode::HTTP_VERSION_NOT_SUPPORTED,
                "HTTP Version Not Supported",
            )),
        }
    }
}

/// Parsed HTTP/1.x request together with connection persistence decision.
pub(crate) struct ParsedRequest {
    pub(crate) request: HttpRequest,
    pub(crate) keep_alive: bool,
}

/// Buffered incremental reader preserving leftover bytes across pipelined HTTP/1.1 requests.
pub(crate) struct ConnReader {
    io: BoxStreamIo,
    buf: Vec<u8>,
    pos: usize,
    eof: bool,
}

impl ConnReader {
    pub(crate) fn new(io: BoxStreamIo) -> Self {
        Self {
            io,
            buf: Vec::with_capacity(8192),
            pos: 0,
            eof: false,
        }
    }

    pub(crate) fn into_io(self) -> BoxStreamIo {
        self.io
    }

    pub(crate) fn io_mut(&mut self) -> &mut BoxStreamIo {
        &mut self.io
    }

    fn available(&self) -> &[u8] {
        &self.buf[self.pos..]
    }

    fn consume(&mut self, n: usize) {
        self.pos += n;
        if self.pos == self.buf.len() {
            self.buf.clear();
            self.pos = 0;
        } else if self.pos >= 4096 {
            self.buf.copy_within(self.pos.., 0);
            self.buf.truncate(self.buf.len() - self.pos);
            self.pos = 0;
        }
    }

    async fn fill_more(&mut self, timeout: Duration) -> io::Result<usize> {
        if self.eof {
            return Ok(0);
        }
        let mut tmp = [0u8; 4096];
        let fut = poll_fn(|cx| Pin::new(&mut *self.io).poll_read(cx, &mut tmp));
        let res =
            tokio::time::timeout(timeout, fut).await.map_err(|_| {
                io::Error::new(io::ErrorKind::TimedOut, "read idle timeout")
            })??;
        if res == 0 {
            self.eof = true;
        } else {
            self.buf.extend_from_slice(&tmp[..res]);
        }
        Ok(res)
    }

    /// Wait for at least 1 buffered byte, interrupting cleanly if `drain` activates.
    async fn wait_first_byte(
        &mut self,
        timeout: Duration,
        drain: &Arc<dyn DrainSignal>,
    ) -> Result<bool, ParseRejection> {
        if !self.available().is_empty() {
            return Ok(true);
        }
        if drain.is_draining() {
            return Ok(false);
        }
        tokio::select! {
            biased;
            _ = drain.wait() => Ok(false),
            res = self.fill_more(timeout) => {
                match res {
                    Ok(0) | Err(_) => Ok(false),
                    Ok(_) => Ok(true),
                }
            }
        }
    }

    /// Read one `\r\n`-terminated line up to `max_len` bytes (excluding CRLF).
    async fn read_line_bounded(
        &mut self,
        max_len: usize,
        timeout: Duration,
        too_long: ParseRejection,
    ) -> Result<Vec<u8>, ParseRejection> {
        loop {
            if let Some(pos) = find_crlf(self.available()) {
                if pos > max_len {
                    return Err(too_long);
                }
                let line = self.available()[..pos].to_vec();
                self.consume(pos + 2);
                return Ok(line);
            }
            // Reject early if already buffered beyond bound without finding CRLF.
            if self.available().len() > max_len + 2 {
                return Err(too_long);
            }
            match self.fill_more(timeout).await {
                Ok(0) | Err(_) => return Err(ParseRejection::BadRequest),
                Ok(_) => {}
            }
        }
    }

    async fn read_exact_bytes(
        &mut self,
        mut len: usize,
        timeout: Duration,
    ) -> Result<Vec<u8>, ParseRejection> {
        let mut out = Vec::with_capacity(len.min(1024 * 1024));
        while len > 0 {
            if !self.available().is_empty() {
                let take = len.min(self.available().len());
                out.extend_from_slice(&self.available()[..take]);
                self.consume(take);
                len -= take;
                continue;
            }
            match self.fill_more(timeout).await {
                Ok(0) | Err(_) => return Err(ParseRejection::BadRequest),
                Ok(_) => {}
            }
        }
        Ok(out)
    }

    /// Parse the next HTTP/1.x request from the connection.
    /// Returns `Ok(None)` on clean EOF or graceful idle drain shutdown.
    pub(crate) async fn read_request(
        &mut self,
        config: &HttpConfig,
        remote_addr: SocketAddr,
        drain: &Arc<dyn DrainSignal>,
    ) -> Result<Option<ParsedRequest>, ParseRejection> {
        if !self.wait_first_byte(config.idle_timeout, drain).await? {
            return Ok(None);
        }

        // Skip optional leading empty CRLF per RFC 9112 Section 2.2 (at most a couple).
        let req_line = loop {
            let line = self
                .read_line_bounded(
                    config.max_request_line_bytes,
                    config.idle_timeout,
                    ParseRejection::UriTooLong,
                )
                .await?;
            if !line.is_empty() {
                break line;
            }
        };

        let (method, raw_uri, version) = parse_request_line(&req_line)?;

        let mut headers = Headers::new();
        let mut header_count = 0usize;
        let mut header_bytes = 0usize;

        loop {
            let remaining =
                config.max_header_bytes.saturating_sub(header_bytes);
            let line = self
                .read_line_bounded(
                    remaining,
                    config.idle_timeout,
                    ParseRejection::HeaderFieldsTooLarge,
                )
                .await?;
            if line.is_empty() {
                break;
            }
            header_count += 1;
            header_bytes += line.len() + 2;
            if header_count > config.max_headers
                || header_bytes > config.max_header_bytes
            {
                return Err(ParseRejection::HeaderFieldsTooLarge);
            }
            let (name, value) = parse_header_line(&line)?;
            headers.append(name, value);
        }

        let body = self
            .read_body(&headers, config.max_body_bytes, config.idle_timeout)
            .await?;

        let keep_alive =
            compute_keep_alive(version, &headers, drain.is_draining());
        let (path, query) = split_uri_target(&raw_uri);

        Ok(Some(ParsedRequest {
            request: HttpRequest {
                method,
                raw_uri,
                path,
                query,
                version,
                headers,
                body,
                remote_addr: Some(remote_addr),
            },
            keep_alive,
        }))
    }

    async fn read_body(
        &mut self,
        headers: &Headers,
        max_body_bytes: usize,
        timeout: Duration,
    ) -> Result<BodyKind, ParseRejection> {
        let has_te = headers.contains("transfer-encoding");
        let cls: Vec<&str> = headers.get_all("content-length").collect();

        if has_te && !cls.is_empty() {
            return Err(ParseRejection::BadRequest);
        }

        if has_te {
            let is_chunked = headers
                .get_all("transfer-encoding")
                .flat_map(|v| v.split(','))
                .last()
                .is_some_and(|last| {
                    last.trim().eq_ignore_ascii_case("chunked")
                });
            if !is_chunked {
                return Err(ParseRejection::BadRequest);
            }
            let bytes = self.read_chunked_body(max_body_bytes, timeout).await?;
            return Ok(if bytes.is_empty() {
                BodyKind::Empty
            } else {
                BodyKind::Bytes(bytes)
            });
        }

        if !cls.is_empty() {
            let mut parsed_len: Option<usize> = None;
            for raw in cls {
                let len = raw
                    .trim()
                    .parse::<usize>()
                    .map_err(|_| ParseRejection::BadRequest)?;
                if let Some(prev) = parsed_len {
                    if prev != len {
                        return Err(ParseRejection::BadRequest);
                    }
                } else {
                    parsed_len = Some(len);
                }
            }
            let len = parsed_len.unwrap_or(0);
            if len > max_body_bytes {
                return Err(ParseRejection::PayloadTooLarge);
            }
            if len == 0 {
                return Ok(BodyKind::Empty);
            }
            let bytes = self.read_exact_bytes(len, timeout).await?;
            return Ok(BodyKind::Bytes(bytes));
        }

        Ok(BodyKind::Empty)
    }

    async fn read_chunked_body(
        &mut self,
        max_body_bytes: usize,
        timeout: Duration,
    ) -> Result<Vec<u8>, ParseRejection> {
        let mut out = Vec::new();
        loop {
            let size_line = self
                .read_line_bounded(1024, timeout, ParseRejection::BadRequest)
                .await?;
            let size_str = std::str::from_utf8(&size_line)
                .map_err(|_| ParseRejection::BadRequest)?
                .split(';')
                .next()
                .unwrap_or("")
                .trim();
            if size_str.is_empty() {
                return Err(ParseRejection::BadRequest);
            }
            let size = usize::from_str_radix(size_str, 16)
                .map_err(|_| ParseRejection::BadRequest)?;
            if size == 0 {
                loop {
                    let trailer = self
                        .read_line_bounded(
                            8192,
                            timeout,
                            ParseRejection::HeaderFieldsTooLarge,
                        )
                        .await?;
                    if trailer.is_empty() {
                        return Ok(out);
                    }
                }
            }
            if out.len().saturating_add(size) > max_body_bytes {
                return Err(ParseRejection::PayloadTooLarge);
            }
            let chunk = self.read_exact_bytes(size, timeout).await?;
            out.extend_from_slice(&chunk);
            let crlf = self.read_exact_bytes(2, timeout).await?;
            if crlf != b"\r\n" {
                return Err(ParseRejection::BadRequest);
            }
        }
    }
}

fn find_crlf(haystack: &[u8]) -> Option<usize> {
    haystack.windows(2).position(|w| w == b"\r\n")
}

fn parse_request_line(
    line: &[u8],
) -> Result<(Method, String, Version), ParseRejection> {
    let text =
        std::str::from_utf8(line).map_err(|_| ParseRejection::BadRequest)?;
    let mut parts = text.split(' ');
    let method_str = parts.next().ok_or(ParseRejection::BadRequest)?;
    let target = parts.next().ok_or(ParseRejection::BadRequest)?;
    let ver_str = parts.next().ok_or(ParseRejection::BadRequest)?;
    if parts.next().is_some() || target.is_empty() {
        return Err(ParseRejection::BadRequest);
    }

    let method = Method::from_bytes(method_str.as_bytes())
        .ok_or(ParseRejection::BadRequest)?;
    let version = match ver_str {
        "HTTP/1.1" => Version::Http11,
        "HTTP/1.0" => Version::Http10,
        s if s.starts_with("HTTP/") => {
            return Err(ParseRejection::HttpVersionNotSupported);
        }
        _ => return Err(ParseRejection::BadRequest),
    };

    let normalized_uri = normalize_target(target)?;
    Ok((method, normalized_uri, version))
}

fn normalize_target(target: &str) -> Result<String, ParseRejection> {
    if target == "*" || target.starts_with('/') {
        return Ok(target.to_owned());
    }
    if let Some(after_scheme) = target
        .strip_prefix("http://")
        .or_else(|| target.strip_prefix("https://"))
    {
        return match after_scheme.find('/') {
            Some(idx) => Ok(after_scheme[idx..].to_owned()),
            None => Ok("/".to_owned()),
        };
    }
    Err(ParseRejection::BadRequest)
}

fn parse_header_line(line: &[u8]) -> Result<(String, String), ParseRejection> {
    let text =
        std::str::from_utf8(line).map_err(|_| ParseRejection::BadRequest)?;
    let (raw_name, raw_val) =
        text.split_once(':').ok_or(ParseRejection::BadRequest)?;
    if raw_name.is_empty() || !raw_name.bytes().all(is_header_name_byte) {
        return Err(ParseRejection::BadRequest);
    }
    let val = raw_val.trim_matches(|c| c == ' ' || c == '\t');
    Ok((raw_name.to_owned(), val.to_owned()))
}

const fn is_header_name_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric()
        || matches!(
            b,
            b'!' | b'#'
                | b'$'
                | b'%'
                | b'&'
                | b'\''
                | b'*'
                | b'+'
                | b'-'
                | b'.'
                | b'^'
                | b'_'
                | b'`'
                | b'|'
                | b'~'
        )
}

fn compute_keep_alive(
    version: Version,
    headers: &Headers,
    draining: bool,
) -> bool {
    if draining {
        return false;
    }
    let mut has_close = false;
    let mut has_keep_alive = false;
    for val in headers.get_all("connection") {
        for tok in val.split(',') {
            let t = tok.trim();
            if t.eq_ignore_ascii_case("close") {
                has_close = true;
            } else if t.eq_ignore_ascii_case("keep-alive") {
                has_keep_alive = true;
            }
        }
    }
    match version {
        Version::Http11 => !has_close,
        Version::Http10 => has_keep_alive && !has_close,
    }
}
