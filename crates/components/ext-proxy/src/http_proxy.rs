//! Layer 3 L7 HTTP/1.1 reverse proxy handler with keep-alive pooling, header
//! rewriting, retry/failover, SSE zero-buffer passthrough, and WS upgrade handoff.

use crate::lb::{ActiveConnGuard, BackendState, UpstreamState};
use crate::pool::dial_backend;
use crate::ws_tunnel::{UpstreamTunnelState, WsTunnelHandler};
use frame::{BoxFuture, Context};
use http_api::{
    AsyncByteStream, BodyKind, Headers, HttpHandler, HttpRequest, HttpResponse,
    Method, StatusCode,
};
use net_api::BoxStreamIo;
use proxy_api::HttpProxyRouteConfig;
use std::fmt::Write as _;
use std::future::poll_fn;
use std::io;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context as TaskContext, Poll};
use std::time::Duration;
use tls_api::TlsConnectorService;
use tokio::sync::mpsc;

const HOP_BY_HOP: &[&str] = &[
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
];

struct MpscBodyStream(mpsc::Receiver<io::Result<Vec<u8>>>);

impl AsyncByteStream for MpscBodyStream {
    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
    ) -> Poll<Option<io::Result<Vec<u8>>>> {
        self.0.poll_recv(cx)
    }
}

pub(crate) struct HttpProxyHandler {
    pub(crate) route: HttpProxyRouteConfig,
    pub(crate) upstream: Arc<UpstreamState>,
    pub(crate) ctx: Context,
}

impl HttpHandler for HttpProxyHandler {
    fn call(&self, req: HttpRequest) -> BoxFuture<'static, HttpResponse> {
        let route = self.route.clone();
        let upstream = Arc::clone(&self.upstream);
        let ctx = self.ctx.clone();
        Box::pin(async move { forward_http(route, upstream, ctx, req).await })
    }
}

fn is_ws_upgrade(headers: &Headers) -> bool {
    let has_upgrade_conn = headers
        .get_all("connection")
        .flat_map(|v| v.split(','))
        .any(|tok| tok.trim().eq_ignore_ascii_case("upgrade"));
    let has_ws = headers
        .get_all("upgrade")
        .flat_map(|v| v.split(','))
        .any(|tok| tok.trim().eq_ignore_ascii_case("websocket"));
    has_upgrade_conn && has_ws
}

fn compute_target_uri(
    req: &HttpRequest,
    route: &HttpProxyRouteConfig,
) -> String {
    let stripped = if let Some(prefix) = &route.strip_prefix {
        let p = prefix.trim_end_matches('/');
        if !p.is_empty() && req.path.starts_with(p) {
            let rest = &req.path[p.len()..];
            if rest.is_empty() {
                "/"
            } else if rest.starts_with('/') {
                rest
            } else {
                req.path.as_str()
            }
        } else {
            req.path.as_str()
        }
    } else {
        req.path.as_str()
    };
    match &req.query {
        Some(q) if !q.is_empty() => format!("{stripped}?{q}"),
        _ => stripped.to_owned(),
    }
}

fn build_request_head(
    req: &HttpRequest,
    route: &HttpProxyRouteConfig,
    backend_host: &str,
    want_ws: bool,
    body_len: Option<u64>,
    chunked_req: bool,
) -> Vec<u8> {
    let target = compute_target_uri(req, route);
    let mut head = String::with_capacity(512);
    let _ = write!(head, "{} {} HTTP/1.1\r\n", req.method.as_str(), target);

    let orig_host = req.headers.get("host");
    if route.preserve_host
        && let Some(h) = orig_host
    {
        let _ = write!(head, "Host: {h}\r\n");
    } else {
        let _ = write!(head, "Host: {backend_host}\r\n");
    }

    for (name, val) in req.headers.iter() {
        let lower = name.to_ascii_lowercase();
        if lower == "host" {
            continue;
        }
        if HOP_BY_HOP.contains(&lower.as_str())
            && !(want_ws && (lower == "connection" || lower == "upgrade"))
        {
            continue;
        }
        if (lower == "content-length" || lower == "transfer-encoding")
            && (body_len.is_some() || chunked_req)
        {
            continue;
        }
        let _ = write!(head, "{name}: {val}\r\n");
    }

    if want_ws {
        if !req.headers.contains("connection") {
            head.push_str("Connection: Upgrade\r\n");
        }
        if !req.headers.contains("upgrade") {
            head.push_str("Upgrade: websocket\r\n");
        }
    } else {
        head.push_str("Connection: keep-alive\r\n");
    }

    if chunked_req {
        head.push_str("Transfer-Encoding: chunked\r\n");
    } else if let Some(len) = body_len
        && (len > 0
            || matches!(req.method, Method::Post | Method::Put | Method::Patch))
    {
        let _ = write!(head, "Content-Length: {len}\r\n");
    }

    let client_ip = req
        .remote_addr
        .map_or_else(|| "127.0.0.1".to_string(), |a| a.ip().to_string());
    let xff = match req.headers.get("x-forwarded-for") {
        Some(prev) if !prev.is_empty() => format!("{prev}, {client_ip}"),
        _ => client_ip.clone(),
    };
    let _ = write!(head, "X-Forwarded-For: {xff}\r\n");
    if !req.headers.contains("x-forwarded-proto") {
        head.push_str("X-Forwarded-Proto: http\r\n");
    }
    if let Some(h) = orig_host
        && !req.headers.contains("x-forwarded-host")
    {
        let _ = write!(head, "X-Forwarded-Host: {h}\r\n");
    }
    let _ = write!(head, "Forwarded: for={client_ip};proto=http\r\n");

    for (ek, ev) in &route.extra_headers {
        let _ = write!(head, "{ek}: {ev}\r\n");
    }

    head.push_str("\r\n");
    head.into_bytes()
}

struct UpstreamReader {
    io: BoxStreamIo,
    buf: Vec<u8>,
    pos: usize,
    eof: bool,
}

impl UpstreamReader {
    fn new(io: BoxStreamIo) -> Self {
        Self {
            io,
            buf: Vec::with_capacity(8192),
            pos: 0,
            eof: false,
        }
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

    fn into_parts(self) -> (BoxStreamIo, Vec<u8>) {
        let leftover = self.buf[self.pos..].to_vec();
        (self.io, leftover)
    }

    async fn fill(&mut self, timeout: Duration) -> io::Result<usize> {
        if self.eof {
            return Ok(0);
        }
        let mut tmp = [0u8; 4096];
        let fut = poll_fn(|cx| Pin::new(&mut *self.io).poll_read(cx, &mut tmp));
        let n = tokio::time::timeout(timeout, fut).await.map_err(|_| {
            io::Error::new(io::ErrorKind::TimedOut, "upstream read timeout")
        })??;
        if n == 0 {
            self.eof = true;
        } else {
            self.buf.extend_from_slice(&tmp[..n]);
        }
        Ok(n)
    }

    async fn read_line(
        &mut self,
        timeout: Duration,
    ) -> io::Result<Option<String>> {
        loop {
            if let Some(pos) =
                self.available().windows(2).position(|w| w == b"\r\n")
            {
                let line = String::from_utf8_lossy(&self.available()[..pos])
                    .into_owned();
                self.consume(pos + 2);
                return Ok(Some(line));
            }
            if self.available().len() > 64 * 1024 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "upstream header line too long",
                ));
            }
            if self.fill(timeout).await? == 0 {
                if self.available().is_empty() {
                    return Ok(None);
                }
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "truncated upstream line",
                ));
            }
        }
    }

    async fn read_exact_bytes(
        &mut self,
        mut len: usize,
        timeout: Duration,
    ) -> io::Result<Vec<u8>> {
        let mut out = Vec::with_capacity(len.min(1024 * 1024));
        while len > 0 {
            if !self.available().is_empty() {
                let take = len.min(self.available().len());
                out.extend_from_slice(&self.available()[..take]);
                self.consume(take);
                len -= take;
                continue;
            }
            if self.fill(timeout).await? == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "truncated upstream body",
                ));
            }
        }
        Ok(out)
    }

    async fn read_response_head(
        &mut self,
        timeout: Duration,
    ) -> io::Result<(StatusCode, Headers)> {
        let Some(status_line) = self.read_line(timeout).await? else {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "upstream closed before status line",
            ));
        };
        let code: u16 = status_line
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("malformed upstream status line: {status_line}"),
                )
            })?;
        let mut headers = Headers::new();
        loop {
            let Some(line) = self.read_line(timeout).await? else {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "truncated upstream headers",
                ));
            };
            if line.is_empty() {
                break;
            }
            if let Some((k, v)) = line.split_once(':') {
                headers.append(k.trim().to_owned(), v.trim().to_owned());
            }
        }
        Ok((StatusCode(code), headers))
    }
}

async fn write_all_raw(io: &mut BoxStreamIo, data: &[u8]) -> io::Result<()> {
    let mut off = 0;
    while off < data.len() {
        let n = poll_fn(|cx| Pin::new(&mut **io).poll_write(cx, &data[off..]))
            .await?;
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "upstream write zero",
            ));
        }
        off += n;
    }
    Ok(())
}

async fn flush_raw(io: &mut BoxStreamIo) -> io::Result<()> {
    poll_fn(|cx| Pin::new(&mut **io).poll_flush(cx)).await
}

async fn send_request_wire(
    io: &mut BoxStreamIo,
    head_bytes: &[u8],
    body: BodyKind,
) -> io::Result<()> {
    write_all_raw(io, head_bytes).await?;
    match body {
        BodyKind::Empty | BodyKind::Upgrade(_) => {}
        BodyKind::Bytes(bytes) => {
            if !bytes.is_empty() {
                write_all_raw(io, &bytes).await?;
            }
        }
        BodyKind::Stream {
            mut reader,
            content_length,
            ..
        } => {
            let use_chunked = content_length.is_none();
            loop {
                let next =
                    poll_fn(|cx| Pin::new(&mut *reader).poll_next(cx)).await;
                match next {
                    Some(Ok(chunk)) => {
                        if chunk.is_empty() {
                            continue;
                        }
                        if use_chunked {
                            let hdr = format!("{:x}\r\n", chunk.len());
                            write_all_raw(io, hdr.as_bytes()).await?;
                            write_all_raw(io, &chunk).await?;
                            write_all_raw(io, b"\r\n").await?;
                        } else {
                            write_all_raw(io, &chunk).await?;
                        }
                    }
                    Some(Err(e)) => return Err(e),
                    None => {
                        if use_chunked {
                            write_all_raw(io, b"0\r\n\r\n").await?;
                        }
                        break;
                    }
                }
            }
        }
    }
    flush_raw(io).await
}

fn header_keep_alive(headers: &Headers) -> bool {
    !headers
        .get_all("connection")
        .flat_map(|v| v.split(','))
        .any(|t| t.trim().eq_ignore_ascii_case("close"))
}

fn filter_response_headers(raw: &Headers, keep_upgrade: bool) -> Headers {
    let mut out = Headers::with_capacity(raw.len());
    for (k, v) in raw.iter() {
        let lower = k.to_ascii_lowercase();
        if HOP_BY_HOP.contains(&lower.as_str())
            && !(keep_upgrade && (lower == "connection" || lower == "upgrade"))
        {
            continue;
        }
        out.append(k.to_owned(), v.to_owned());
    }
    out
}

async fn pump_chunked_upstream(
    mut reader: UpstreamReader,
    tx: mpsc::Sender<io::Result<Vec<u8>>>,
    backend: Arc<BackendState>,
    cap: usize,
    keep_alive: bool,
    _guard: ActiveConnGuard,
) {
    let read_timeout = Duration::from_secs(300);
    let res: io::Result<bool> = async {
        loop {
            let Some(size_line) = reader.read_line(read_timeout).await? else {
                return Ok(false);
            };
            let size_hex = size_line.split(';').next().unwrap_or("").trim();
            let size = usize::from_str_radix(size_hex, 16).map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("bad upstream chunk size `{size_line}`"),
                )
            })?;
            if size == 0 {
                loop {
                    let Some(tr) = reader.read_line(read_timeout).await? else {
                        return Ok(false);
                    };
                    if tr.is_empty() {
                        return Ok(true);
                    }
                }
            }
            let mut rem = size;
            while rem > 0 {
                if reader.available().is_empty()
                    && reader.fill(read_timeout).await? == 0
                {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "eof in chunk payload",
                    ));
                }
                let take = rem.min(reader.available().len());
                let chunk = reader.available()[..take].to_vec();
                reader.consume(take);
                rem -= take;
                if tx.send(Ok(chunk)).await.is_err() {
                    return Ok(false);
                }
            }
            let crlf = reader.read_exact_bytes(2, read_timeout).await?;
            if crlf != b"\r\n" {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "missing chunk CRLF",
                ));
            }
        }
    }
    .await;

    match res {
        Ok(clean) => {
            if clean && keep_alive {
                let (io, leftover) = reader.into_parts();
                if leftover.is_empty() {
                    backend.idle.push(io, cap);
                }
            }
        }
        Err(e) => {
            let _ = tx.send(Err(e)).await;
        }
    }
}

async fn pump_fixed_or_eof_upstream(
    mut reader: UpstreamReader,
    mut remaining: Option<u64>,
    tx: mpsc::Sender<io::Result<Vec<u8>>>,
    backend: Arc<BackendState>,
    cap: usize,
    keep_alive: bool,
    _guard: ActiveConnGuard,
) {
    let read_timeout = Duration::from_secs(300);
    let clean = loop {
        if let Some(0) = remaining {
            break true;
        }
        if reader.available().is_empty() {
            match reader.fill(read_timeout).await {
                Ok(0) => {
                    break remaining.is_none();
                }
                Ok(_) => {}
                Err(e) => {
                    let _ = tx.send(Err(e)).await;
                    break false;
                }
            }
        }
        let avail = reader.available().len();
        let take = match remaining {
            Some(r) => (avail as u64).min(r) as usize,
            None => avail,
        };
        if take == 0 {
            break true;
        }
        let chunk = reader.available()[..take].to_vec();
        reader.consume(take);
        if let Some(ref mut r) = remaining {
            *r -= take as u64;
        }
        if tx.send(Ok(chunk)).await.is_err() {
            break false;
        }
    };

    if clean && keep_alive && remaining.is_some() {
        let (io, leftover) = reader.into_parts();
        if leftover.is_empty() {
            backend.idle.push(io, cap);
        }
    }
}

async fn forward_http(
    route: HttpProxyRouteConfig,
    upstream: Arc<UpstreamState>,
    ctx: Context,
    mut req: HttpRequest,
) -> HttpResponse {
    let want_ws = route.websocket && is_ws_upgrade(&req.headers);
    let can_retry_backend = route.retry_idempotent
        && !want_ws
        && matches!(req.method, Method::Get | Method::Head)
        && !matches!(req.body, BodyKind::Stream { .. });

    let (body_len, chunked_req) = match &req.body {
        BodyKind::Empty | BodyKind::Upgrade(_) => (Some(0), false),
        BodyKind::Bytes(b) => (Some(b.len() as u64), false),
        BodyKind::Stream { content_length, .. } => {
            (*content_length, content_length.is_none())
        }
    };

    // Snapshot small buffered body if retry across backends might be needed.
    let retry_body_copy = if can_retry_backend {
        if let BodyKind::Bytes(ref b) = req.body {
            Some(b.clone())
        } else {
            None
        }
    } else {
        None
    };

    let mut body_opt = Some(std::mem::replace(&mut req.body, BodyKind::Empty));
    let mut skip_idx: Option<usize> = None;
    let max_attempts = if can_retry_backend { 2 } else { 1 };
    let tls = ctx.get::<TlsConnectorService>();

    for attempt in 0..max_attempts {
        let Some((idx, backend, guard)) = upstream.select_backend(skip_idx)
        else {
            return HttpResponse::text(
                StatusCode::SERVICE_UNAVAILABLE,
                "No healthy upstream backend available",
            );
        };
        skip_idx = Some(idx);

        let backend_host = backend
            .cfg
            .sni
            .clone()
            .unwrap_or_else(|| backend.cfg.addr.to_string());
        let head_bytes = build_request_head(
            &req,
            &route,
            &backend_host,
            want_ws,
            body_len,
            chunked_req,
        );

        let pooled = if want_ws {
            None
        } else {
            backend.idle.pop_fresh()
        };
        let was_pooled = pooled.is_some();
        let mut conn_res = match pooled {
            Some(io) => Ok(io),
            None => {
                dial_backend(
                    &backend.cfg,
                    upstream.cfg.connect_timeout_ms,
                    tls.as_deref(),
                )
                .await
            }
        };

        let mut body_for_send = if attempt == 0 {
            body_opt.take().unwrap_or(BodyKind::Empty)
        } else {
            match &retry_body_copy {
                Some(b) => BodyKind::Bytes(b.clone()),
                None => BodyKind::Empty,
            }
        };

        let mut retried_stale = false;
        let head_res = loop {
            let mut io = match conn_res {
                Ok(io) => io,
                Err(e) => break Err(e),
            };
            let body_to_send =
                std::mem::replace(&mut body_for_send, BodyKind::Empty);
            if let Err(e) =
                send_request_wire(&mut io, &head_bytes, body_to_send).await
            {
                if was_pooled
                    && !retried_stale
                    && !matches!(req.body, BodyKind::Stream { .. })
                {
                    retried_stale = true;
                    body_for_send = match &retry_body_copy {
                        Some(b) => BodyKind::Bytes(b.clone()),
                        None => BodyKind::Empty,
                    };
                    conn_res = dial_backend(
                        &backend.cfg,
                        upstream.cfg.connect_timeout_ms,
                        tls.as_deref(),
                    )
                    .await;
                    continue;
                }
                break Err(e);
            }

            let mut reader = UpstreamReader::new(io);
            let hdr_timeout = Duration::from_millis(
                upstream.cfg.response_header_timeout_ms.max(1),
            );
            match reader.read_response_head(hdr_timeout).await {
                Ok((status, resp_headers)) => {
                    break Ok((reader, status, resp_headers));
                }
                Err(e) => {
                    if was_pooled
                        && !retried_stale
                        && !matches!(req.body, BodyKind::Stream { .. })
                    {
                        retried_stale = true;
                        body_for_send = match &retry_body_copy {
                            Some(b) => BodyKind::Bytes(b.clone()),
                            None => BodyKind::Empty,
                        };
                        conn_res = dial_backend(
                            &backend.cfg,
                            upstream.cfg.connect_timeout_ms,
                            tls.as_deref(),
                        )
                        .await;
                        continue;
                    }
                    break Err(e);
                }
            }
        };

        let (mut reader, status, raw_headers) = match head_res {
            Ok(ok) => ok,
            Err(e) => {
                drop(guard);
                backend.record_passive_failure(
                    upstream.cfg.passive_eject_threshold,
                    upstream.cfg.passive_eject_ms,
                );
                if attempt + 1 < max_attempts {
                    continue;
                }
                let code = if e.kind() == io::ErrorKind::TimedOut {
                    StatusCode::GATEWAY_TIMEOUT
                } else {
                    StatusCode::BAD_GATEWAY
                };
                return HttpResponse::text(
                    code,
                    format!("Upstream error: {e}"),
                );
            }
        };

        backend.record_success();

        // 1. WebSocket 101 Switching Protocols tunnel handoff
        if want_ws && status == StatusCode::SWITCHING_PROTOCOLS {
            let headers = filter_response_headers(&raw_headers, true);
            let (upstream_io, upstream_leftover) = reader.into_parts();
            let tunnel = Arc::new(WsTunnelHandler {
                state: Mutex::new(Some(UpstreamTunnelState {
                    upstream_io,
                    upstream_leftover,
                    guard,
                })),
            });
            return HttpResponse::upgrade(status, headers, tunnel);
        }

        let headers = filter_response_headers(&raw_headers, false);
        let upstream_keep_alive = header_keep_alive(&raw_headers);
        let pool_cap = upstream.cfg.max_idle_per_backend;

        // 2. Empty body on HEAD / 204 / 304 / 1xx
        if req.method == Method::Head
            || status.is_informational()
            || status == StatusCode::NO_CONTENT
            || status == StatusCode::NOT_MODIFIED
        {
            if upstream_keep_alive {
                let (io, leftover) = reader.into_parts();
                if leftover.is_empty() {
                    backend.idle.push(io, pool_cap);
                }
            }
            return HttpResponse {
                status,
                headers,
                body: BodyKind::Empty,
            };
        }

        let is_chunked = raw_headers
            .get_all("transfer-encoding")
            .flat_map(|v| v.split(','))
            .any(|t| t.trim().eq_ignore_ascii_case("chunked"));
        let is_sse = raw_headers.get("content-type").is_some_and(|ct| {
            ct.to_ascii_lowercase().starts_with("text/event-stream")
        });

        // 3. Chunked / SSE zero-buffer stream
        if is_chunked {
            let (tx, rx) = mpsc::channel(16);
            tokio::spawn(pump_chunked_upstream(
                reader,
                tx,
                backend,
                pool_cap,
                upstream_keep_alive,
                guard,
            ));
            return HttpResponse {
                status,
                headers,
                body: BodyKind::Stream {
                    reader: Box::new(MpscBodyStream(rx)),
                    content_length: None,
                    streaming: true,
                },
            };
        }

        let content_len = raw_headers
            .get("content-length")
            .and_then(|s| s.trim().parse::<u64>().ok());

        match content_len {
            Some(0) => {
                if upstream_keep_alive {
                    let (io, leftover) = reader.into_parts();
                    if leftover.is_empty() {
                        backend.idle.push(io, pool_cap);
                    }
                }
                return HttpResponse {
                    status,
                    headers,
                    body: BodyKind::Empty,
                };
            }
            Some(len) if len <= 64 * 1024 && !is_sse => {
                let bytes = match reader
                    .read_exact_bytes(len as usize, Duration::from_secs(60))
                    .await
                {
                    Ok(b) => b,
                    Err(e) => {
                        return HttpResponse::text(
                            StatusCode::BAD_GATEWAY,
                            format!("Upstream body error: {e}"),
                        );
                    }
                };
                if upstream_keep_alive {
                    let (io, leftover) = reader.into_parts();
                    if leftover.is_empty() {
                        backend.idle.push(io, pool_cap);
                    }
                }
                return HttpResponse {
                    status,
                    headers,
                    body: BodyKind::Bytes(bytes),
                };
            }
            other_len => {
                let (tx, rx) = mpsc::channel(16);
                let streaming = is_sse || other_len.is_none();
                tokio::spawn(pump_fixed_or_eof_upstream(
                    reader,
                    other_len,
                    tx,
                    backend,
                    pool_cap,
                    upstream_keep_alive && other_len.is_some(),
                    guard,
                ));
                return HttpResponse {
                    status,
                    headers,
                    body: BodyKind::Stream {
                        reader: Box::new(MpscBodyStream(rx)),
                        content_length: other_len,
                        streaming,
                    },
                };
            }
        }
    }

    HttpResponse::text(StatusCode::BAD_GATEWAY, "All upstream attempts failed")
}
