//! Layer 3 streaming-aware response compression filter plugin.

use brotli::CompressorWriter;
use flate2::Compression;
use flate2::write::{GzEncoder, ZlibEncoder};
use frame::{
    BoxFuture, ConfigFieldSchema, ConfigFieldType, ConfigValue, Context, Deps,
    FrameError, Plugin, PluginSchema, ValidationIssue,
};
use http_api::{
    AsyncByteStream, BodyKind, FilterOutcome, HttpFilter, HttpRequest,
    HttpResponse, RequestMeta, RouterService, StatusCode,
};
use std::future::poll_fn;
use std::io::{self, Write};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context as TaskContext, Poll};

/// Configuration for [`CompressionPlugin`].
#[derive(Debug, Clone)]
pub struct CompressionConfig {
    /// Minimum static byte body length before compression activates.
    pub min_size: usize,
}

impl Default for CompressionConfig {
    fn default() -> Self {
        Self { min_size: 256 }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Encoding {
    Br,
    Gzip,
    Deflate,
}

impl Encoding {
    const fn token(self) -> &'static str {
        match self {
            Self::Br => "br",
            Self::Gzip => "gzip",
            Self::Deflate => "deflate",
        }
    }

    const fn rank(self) -> u8 {
        match self {
            Self::Br => 3,
            Self::Gzip => 2,
            Self::Deflate => 1,
        }
    }
}

fn negotiate_encoding(req_meta: &RequestMeta) -> Option<Encoding> {
    let mut best: Option<(u32, u8, Encoding)> = None;
    let mut star_q: Option<u32> = None;
    let mut explicit_zero = [false; 3]; // br, gzip, deflate

    for header in req_meta.headers.get_all("accept-encoding") {
        for raw_item in header.split(',') {
            let mut parts = raw_item.split(';');
            let Some(name_raw) = parts.next() else {
                continue;
            };
            let name = name_raw.trim().to_ascii_lowercase();
            if name.is_empty() {
                continue;
            }
            let mut q: u32 = 1000;
            for param in parts {
                if let Some((k, v)) = param.trim().split_once('=')
                    && k.trim().eq_ignore_ascii_case("q")
                {
                    q = parse_qvalue(v.trim()).unwrap_or(0);
                }
            }
            if name == "*" {
                star_q = Some(q);
                continue;
            }
            let enc = match name.as_str() {
                "br" => Some((Encoding::Br, 0usize)),
                "gzip" | "x-gzip" => Some((Encoding::Gzip, 1usize)),
                "deflate" => Some((Encoding::Deflate, 2usize)),
                _ => None,
            };
            if let Some((algo, slot)) = enc {
                if q == 0 {
                    explicit_zero[slot] = true;
                } else {
                    let cand = (q, algo.rank(), algo);
                    if best.is_none_or(|b| (cand.0, cand.1) > (b.0, b.1)) {
                        best = Some(cand);
                    }
                }
            }
        }
    }

    if best.is_none()
        && let Some(sq) = star_q
        && sq > 0
    {
        for (algo, slot) in [
            (Encoding::Br, 0),
            (Encoding::Gzip, 1),
            (Encoding::Deflate, 2),
        ] {
            if !explicit_zero[slot] {
                return Some(algo);
            }
        }
    }

    best.map(|(_, _, algo)| algo)
}

fn parse_qvalue(raw: &str) -> Option<u32> {
    let f: f32 = raw.parse().ok()?;
    if !(0.0..=1.0).contains(&f) {
        return None;
    }
    Some((f * 1000.0).round() as u32)
}

fn is_exempt_content_type(ct: &str) -> bool {
    let media = ct
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    media == "text/event-stream"
        || media.starts_with("video/")
        || media.starts_with("audio/")
        || matches!(
            media.as_str(),
            "image/png"
                | "image/jpeg"
                | "image/jpg"
                | "image/webp"
                | "image/avif"
                | "image/gif"
                | "application/gzip"
                | "application/x-gzip"
                | "application/zip"
                | "application/zstd"
                | "application/x-bzip2"
                | "application/x-xz"
                | "application/br"
                | "font/woff"
                | "font/woff2"
        )
}

fn should_exempt(
    req_meta: &RequestMeta,
    resp: &HttpResponse,
    min_size: usize,
) -> bool {
    if req_meta.is_head
        || resp.status.as_u16() < 200
        || resp.status == StatusCode::PARTIAL_CONTENT
        || resp.status == StatusCode::NOT_MODIFIED
        || resp.headers.contains("content-encoding")
    {
        return true;
    }

    if resp.headers.get_all("cache-control").any(|v| {
        v.split(',')
            .any(|part| part.trim().eq_ignore_ascii_case("no-transform"))
    }) {
        return true;
    }

    if resp
        .headers
        .get("content-type")
        .is_some_and(is_exempt_content_type)
    {
        return true;
    }

    match &resp.body {
        BodyKind::Empty => true,
        BodyKind::Stream {
            streaming: true, ..
        } => true,
        BodyKind::Stream {
            content_length,
            streaming: false,
            ..
        } => content_length.is_some_and(|len| len < min_size as u64),
        BodyKind::Bytes(b) => b.len() < min_size,
    }
}

fn compress_bytes(algo: Encoding, data: &[u8]) -> io::Result<Vec<u8>> {
    let mut out = Vec::new();
    match algo {
        Encoding::Br => {
            let mut enc = CompressorWriter::new(&mut out, 4096, 4, 22);
            enc.write_all(data)?;
            enc.flush()?;
        }
        Encoding::Gzip => {
            let mut enc = GzEncoder::new(Vec::new(), Compression::default());
            enc.write_all(data)?;
            out = enc.finish()?;
        }
        Encoding::Deflate => {
            let mut enc = ZlibEncoder::new(Vec::new(), Compression::default());
            enc.write_all(data)?;
            out = enc.finish()?;
        }
    }
    Ok(out)
}

#[derive(Clone)]
struct SharedOut(Arc<Mutex<Vec<u8>>>);

impl Write for SharedOut {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut guard = self.0.lock().unwrap_or_else(|e| e.into_inner());
        guard.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

enum StreamEncoder {
    Br(Box<CompressorWriter<SharedOut>>),
    Gzip(GzEncoder<SharedOut>),
    Deflate(ZlibEncoder<SharedOut>),
}

impl StreamEncoder {
    fn new(algo: Encoding, sink: SharedOut) -> Self {
        match algo {
            Encoding::Br => {
                Self::Br(Box::new(CompressorWriter::new(sink, 4096, 4, 22)))
            }
            Encoding::Gzip => {
                Self::Gzip(GzEncoder::new(sink, Compression::default()))
            }
            Encoding::Deflate => {
                Self::Deflate(ZlibEncoder::new(sink, Compression::default()))
            }
        }
    }

    fn write_chunk(&mut self, chunk: &[u8]) -> io::Result<()> {
        match self {
            Self::Br(e) => {
                e.write_all(chunk)?;
                e.flush()
            }
            Self::Gzip(e) => {
                e.write_all(chunk)?;
                e.flush()
            }
            Self::Deflate(e) => {
                e.write_all(chunk)?;
                e.flush()
            }
        }
    }

    fn finish(self) -> io::Result<()> {
        match self {
            Self::Br(mut e) => e.flush(),
            Self::Gzip(e) => {
                e.finish()?;
                Ok(())
            }
            Self::Deflate(e) => {
                e.finish()?;
                Ok(())
            }
        }
    }
}

struct CompressedStream {
    inner: Box<dyn AsyncByteStream>,
    sink: Arc<Mutex<Vec<u8>>>,
    encoder: Option<StreamEncoder>,
    finished: bool,
}

impl CompressedStream {
    fn new(inner: Box<dyn AsyncByteStream>, algo: Encoding) -> Self {
        let sink = Arc::new(Mutex::new(Vec::new()));
        let encoder = StreamEncoder::new(algo, SharedOut(Arc::clone(&sink)));
        Self {
            inner,
            sink,
            encoder: Some(encoder),
            finished: false,
        }
    }

    fn take_out(&self) -> Vec<u8> {
        let mut guard = self.sink.lock().unwrap_or_else(|e| e.into_inner());
        std::mem::take(&mut *guard)
    }
}

impl AsyncByteStream for CompressedStream {
    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
    ) -> Poll<Option<io::Result<Vec<u8>>>> {
        let buffered = self.take_out();
        if !buffered.is_empty() {
            return Poll::Ready(Some(Ok(buffered)));
        }
        if self.finished {
            return Poll::Ready(None);
        }

        loop {
            match Pin::new(&mut *self.inner).poll_next(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Some(Err(e))) => {
                    self.finished = true;
                    return Poll::Ready(Some(Err(e)));
                }
                Poll::Ready(Some(Ok(chunk))) => {
                    if let Some(enc) = self.encoder.as_mut()
                        && let Err(e) = enc.write_chunk(&chunk)
                    {
                        self.finished = true;
                        return Poll::Ready(Some(Err(e)));
                    }
                    let out = self.take_out();
                    if !out.is_empty() {
                        return Poll::Ready(Some(Ok(out)));
                    }
                }
                Poll::Ready(None) => {
                    self.finished = true;
                    if let Some(enc) = self.encoder.take()
                        && let Err(e) = enc.finish()
                    {
                        return Poll::Ready(Some(Err(e)));
                    }
                    let tail = self.take_out();
                    if tail.is_empty() {
                        return Poll::Ready(None);
                    }
                    return Poll::Ready(Some(Ok(tail)));
                }
            }
        }
    }
}

fn ensure_vary_accept_encoding(resp: &mut HttpResponse) {
    let already = resp.headers.get_all("vary").any(|v| {
        v.split(',').any(|part| {
            let p = part.trim();
            p == "*" || p.eq_ignore_ascii_case("accept-encoding")
        })
    });
    if !already {
        resp.headers.append("Vary", "Accept-Encoding");
    }
}

fn weaken_strong_etag(resp: &mut HttpResponse) {
    if let Some(etag) = resp.headers.get("etag")
        && etag.starts_with('"')
    {
        let weak = format!("W/{etag}");
        resp.headers.insert("ETag", weak);
    }
}

struct CompressionFilter {
    min_size: usize,
}

impl HttpFilter for CompressionFilter {
    fn name(&self) -> &str {
        "compression"
    }

    fn priority(&self) -> i32 {
        10
    }

    fn on_request(
        &self,
        req: HttpRequest,
    ) -> BoxFuture<'static, FilterOutcome> {
        Box::pin(async move { FilterOutcome::Continue(req) })
    }

    fn on_response(
        &self,
        req_meta: &RequestMeta,
        mut resp: HttpResponse,
    ) -> BoxFuture<'static, HttpResponse> {
        let min_size = self.min_size;
        let meta = req_meta.clone();
        Box::pin(async move {
            if should_exempt(&meta, &resp, min_size) {
                return resp;
            }
            let Some(algo) = negotiate_encoding(&meta) else {
                return resp;
            };

            let orig_body = std::mem::replace(&mut resp.body, BodyKind::Empty);
            match orig_body {
                BodyKind::Bytes(data) => match compress_bytes(algo, &data) {
                    Ok(compressed) => {
                        resp.headers.insert(
                            "Content-Length",
                            compressed.len().to_string(),
                        );
                        resp.headers.insert("Content-Encoding", algo.token());
                        ensure_vary_accept_encoding(&mut resp);
                        weaken_strong_etag(&mut resp);
                        resp.body = BodyKind::Bytes(compressed);
                    }
                    Err(_) => {
                        resp.body = BodyKind::Bytes(data);
                    }
                },
                BodyKind::Stream {
                    reader,
                    content_length: _,
                    streaming: false,
                } => {
                    resp.headers.remove("content-length");
                    resp.headers.insert("Content-Encoding", algo.token());
                    ensure_vary_accept_encoding(&mut resp);
                    weaken_strong_etag(&mut resp);
                    let wrapped = CompressedStream::new(reader, algo);
                    let _ = poll_fn(|cx| {
                        let _ = cx;
                        Poll::Ready(())
                    })
                    .await;
                    resp.body = BodyKind::Stream {
                        reader: Box::new(wrapped),
                        content_length: None,
                        streaming: false,
                    };
                }
                other => {
                    resp.body = other;
                }
            }
            resp
        })
    }
}

/// Plugin mounting the streaming-aware HTTP compression filter.
#[derive(Debug, Clone, Default)]
pub struct CompressionPlugin {
    config: CompressionConfig,
}

impl CompressionPlugin {
    /// Create a compression plugin with explicit configuration.
    pub fn new(config: CompressionConfig) -> Self {
        Self { config }
    }
}

fn resolve_compression_config(
    base: &CompressionConfig,
    raw: &ConfigValue,
) -> Result<CompressionConfig, FrameError> {
    let mut cfg = base.clone();
    match raw {
        ConfigValue::Null => Ok(cfg),
        ConfigValue::Map(map) => {
            let mut issues = Vec::new();
            for (k, v) in map {
                match k.as_str() {
                    "min_size" => match v {
                        ConfigValue::Int(n) if *n >= 0 => {
                            cfg.min_size = *n as usize;
                        }
                        _ => issues.push(ValidationIssue {
                            path: k.clone(),
                            message: "min_size must be a non-negative integer"
                                .to_string(),
                        }),
                    },
                    _ => issues.push(ValidationIssue {
                        path: k.clone(),
                        message: format!(
                            "unknown ext-compression option `{k}`"
                        ),
                    }),
                }
            }
            if issues.is_empty() {
                Ok(cfg)
            } else {
                Err(FrameError::validation(issues))
            }
        }
        _ => Err(FrameError::validation(vec![ValidationIssue {
            path: "".to_string(),
            message: "ext-compression config must be Null or Map".to_string(),
        }])),
    }
}

impl Plugin for CompressionPlugin {
    fn name(&self) -> &str {
        "ext-compression"
    }

    fn deps(&self) -> Deps {
        Deps::empty().with::<RouterService>()
    }

    fn schema(&self) -> PluginSchema {
        PluginSchema::new(
            "Layer 3 streaming-aware br/gzip/deflate compression filter with automatic exemptions for SSE/206/304/media",
        )
        .field(
            ConfigFieldSchema::new(
                "min_size",
                ConfigFieldType::Int,
                "Minimum static response body size in bytes before compression activates",
            )
            .with_default(ConfigValue::Int(self.config.min_size as i64)),
        )
    }

    fn validate_config(
        &self,
        raw: &ConfigValue,
    ) -> Result<ConfigValue, FrameError> {
        let _ = resolve_compression_config(&self.config, raw)?;
        Ok(raw.clone())
    }

    fn apply(
        &self,
        ctx: Context,
        config: ConfigValue,
    ) -> BoxFuture<'static, Result<(), FrameError>> {
        let base = self.config.clone();
        Box::pin(async move {
            let cfg = resolve_compression_config(&base, &config)?;
            let router = ctx.use_service::<RouterService>()?;
            let _ = router.mount_filter(Arc::new(CompressionFilter {
                min_size: cfg.min_size,
            }))?;
            Ok(())
        })
    }
}
