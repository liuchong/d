//! Layer 2 HTTP contracts: protocol values, streaming body primitives,
//! request/response filters, and the snapshot router interface.

use frame::{
    BoxFuture, Context as FrameContext, EffectHandle, FrameError,
    ScopedService, Service,
};
use net_api::{BoxStreamIo, DrainSignal};
use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::future::poll_fn;
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

/// HTTP request method.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Method {
    Get,
    Head,
    Post,
    Put,
    Delete,
    Options,
    Patch,
    Other(String),
}

impl Method {
    /// Return the uppercase wire method name.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Get => "GET",
            Self::Head => "HEAD",
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Delete => "DELETE",
            Self::Options => "OPTIONS",
            Self::Patch => "PATCH",
            Self::Other(s) => s.as_str(),
        }
    }

    /// Parse a method token from wire bytes.
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.is_empty() || !bytes.iter().all(is_tchar) {
            return None;
        }
        let s = std::str::from_utf8(bytes).ok()?;
        Some(match s {
            "GET" => Self::Get,
            "HEAD" => Self::Head,
            "POST" => Self::Post,
            "PUT" => Self::Put,
            "DELETE" => Self::Delete,
            "OPTIONS" => Self::Options,
            "PATCH" => Self::Patch,
            _ => Self::Other(s.to_owned()),
        })
    }
}

impl fmt::Display for Method {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

fn is_tchar(b: &u8) -> bool {
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

/// HTTP response status code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StatusCode(pub u16);

impl StatusCode {
    pub const SWITCHING_PROTOCOLS: Self = Self(101);
    pub const OK: Self = Self(200);
    pub const NO_CONTENT: Self = Self(204);
    pub const PARTIAL_CONTENT: Self = Self(206);
    pub const MOVED_PERMANENTLY: Self = Self(301);
    pub const NOT_MODIFIED: Self = Self(304);
    pub const BAD_REQUEST: Self = Self(400);
    pub const UNAUTHORIZED: Self = Self(401);
    pub const FORBIDDEN: Self = Self(403);
    pub const NOT_FOUND: Self = Self(404);
    pub const METHOD_NOT_ALLOWED: Self = Self(405);
    pub const PAYLOAD_TOO_LARGE: Self = Self(413);
    pub const URI_TOO_LONG: Self = Self(414);
    pub const RANGE_NOT_SATISFIABLE: Self = Self(416);
    pub const UPGRADE_REQUIRED: Self = Self(426);
    pub const TOO_MANY_REQUESTS: Self = Self(429);
    pub const HEADER_FIELDS_TOO_LARGE: Self = Self(431);
    pub const INTERNAL_SERVER_ERROR: Self = Self(500);
    pub const BAD_GATEWAY: Self = Self(502);
    pub const SERVICE_UNAVAILABLE: Self = Self(503);
    pub const GATEWAY_TIMEOUT: Self = Self(504);
    pub const HTTP_VERSION_NOT_SUPPORTED: Self = Self(505);

    /// Return the raw integer status code.
    pub const fn as_u16(self) -> u16 {
        self.0
    }

    /// Return the canonical reason phrase for known status codes.
    pub const fn reason_phrase(self) -> &'static str {
        match self.0 {
            100 => "Continue",
            101 => "Switching Protocols",
            200 => "OK",
            201 => "Created",
            204 => "No Content",
            206 => "Partial Content",
            301 => "Moved Permanently",
            302 => "Found",
            304 => "Not Modified",
            307 => "Temporary Redirect",
            308 => "Permanent Redirect",
            400 => "Bad Request",
            401 => "Unauthorized",
            403 => "Forbidden",
            404 => "Not Found",
            405 => "Method Not Allowed",
            413 => "Payload Too Large",
            414 => "URI Too Long",
            416 => "Range Not Satisfiable",
            426 => "Upgrade Required",
            429 => "Too Many Requests",
            431 => "Request Header Fields Too Large",
            500 => "Internal Server Error",
            502 => "Bad Gateway",
            503 => "Service Unavailable",
            504 => "Gateway Timeout",
            505 => "HTTP Version Not Supported",
            _ => "Unknown",
        }
    }

    /// Return `true` for 1xx informational statuses.
    pub const fn is_informational(self) -> bool {
        self.0 >= 100 && self.0 < 200
    }

    /// Return `true` for 2xx success statuses.
    pub const fn is_success(self) -> bool {
        self.0 >= 200 && self.0 < 300
    }
}

impl fmt::Display for StatusCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Supported HTTP wire protocol versions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Version {
    Http10,
    Http11,
}

impl Version {
    /// Return the HTTP-version token used on the wire.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Http10 => "HTTP/1.0",
            Self::Http11 => "HTTP/1.1",
        }
    }
}

/// Ordered HTTP header map preserving insertion order and case-insensitive lookup.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Headers {
    entries: Vec<(String, String)>,
}

impl Headers {
    /// Create an empty header map.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create an empty header map with preallocated capacity.
    pub fn with_capacity(cap: usize) -> Self {
        Self {
            entries: Vec::with_capacity(cap),
        }
    }

    /// Insert or replace all existing entries matching `name` (case-insensitive).
    pub fn insert(
        &mut self,
        name: impl Into<String>,
        value: impl Into<String>,
    ) {
        let name = name.into();
        let value = value.into();
        self.remove(&name);
        self.entries.push((name, value));
    }

    /// Append a header entry without removing previous entries of the same name.
    pub fn append(
        &mut self,
        name: impl Into<String>,
        value: impl Into<String>,
    ) {
        self.entries.push((name.into(), value.into()));
    }

    /// Return the first value matching `name` (ASCII case-insensitive).
    pub fn get(&self, name: &str) -> Option<&str> {
        self.entries
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// Iterate over all values matching `name` (ASCII case-insensitive).
    pub fn get_all<'a>(
        &'a self,
        name: &'a str,
    ) -> impl Iterator<Item = &'a str> + 'a {
        self.entries
            .iter()
            .filter(move |(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// Remove every entry matching `name` (ASCII case-insensitive).
    pub fn remove(&mut self, name: &str) -> bool {
        let before = self.entries.len();
        self.entries.retain(|(k, _)| !k.eq_ignore_ascii_case(name));
        self.entries.len() != before
    }

    /// Return `true` if at least one entry matches `name` (ASCII case-insensitive).
    pub fn contains(&self, name: &str) -> bool {
        self.entries
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case(name))
    }

    /// Iterate over all `(name, value)` pairs in order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.entries.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }

    /// Return the number of header fields stored.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Return `true` if no headers are stored.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Chunked asynchronous byte stream yielding incremental response or request chunks.
pub trait AsyncByteStream: Send + Unpin + 'static {
    /// Poll for the next non-empty byte chunk, or `None` on clean EOF.
    fn poll_next(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<io::Result<Vec<u8>>>>;
}

/// Create an in-memory push/pull byte stream pair with zero external dependencies.
pub fn stream_channel() -> (StreamSender, Box<dyn AsyncByteStream>) {
    let state = Arc::new(Mutex::new(StreamChannelState {
        queue: VecDeque::new(),
        closed: false,
        waker: None,
    }));
    (
        StreamSender {
            state: Arc::clone(&state),
        },
        Box::new(StreamReceiver { state }),
    )
}

#[derive(Debug)]
struct StreamChannelState {
    queue: VecDeque<io::Result<Vec<u8>>>,
    closed: bool,
    waker: Option<Waker>,
}

/// Producer handle for pushing chunks into a streaming [`BodyKind`].
#[derive(Clone, Debug)]
pub struct StreamSender {
    state: Arc<Mutex<StreamChannelState>>,
}

impl StreamSender {
    /// Push a chunk and wake the consumer immediately.
    pub fn send(&self, chunk: Vec<u8>) -> Result<(), io::Error> {
        let mut guard = self
            .state
            .lock()
            .map_err(|_| io::Error::other("stream channel poisoned"))?;
        if guard.closed {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "stream closed",
            ));
        }
        if !chunk.is_empty() {
            guard.queue.push_back(Ok(chunk));
        }
        if let Some(waker) = guard.waker.take() {
            drop(guard);
            waker.wake();
        }
        Ok(())
    }

    /// Push a terminal stream error and wake the consumer.
    pub fn send_error(&self, err: io::Error) {
        let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        guard.queue.push_back(Err(err));
        guard.closed = true;
        if let Some(waker) = guard.waker.take() {
            drop(guard);
            waker.wake();
        }
    }

    /// Close the stream cleanly so the consumer observes EOF after pending chunks.
    pub fn close(self) {
        let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        guard.closed = true;
        if let Some(waker) = guard.waker.take() {
            drop(guard);
            waker.wake();
        }
    }
}

struct StreamReceiver {
    state: Arc<Mutex<StreamChannelState>>,
}

impl AsyncByteStream for StreamReceiver {
    fn poll_next(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<io::Result<Vec<u8>>>> {
        let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(item) = guard.queue.pop_front() {
            return Poll::Ready(Some(item));
        }
        if guard.closed {
            return Poll::Ready(None);
        }
        guard.waker = Some(cx.waker().clone());
        Poll::Pending
    }
}

/// Post-`101 Switching Protocols` connection ownership handoff callback.
pub trait UpgradeHandler: Send + Sync + 'static {
    /// Take exclusive ownership of the upgraded socket stream, any pre-buffered
    /// bytes already read past HTTP headers, and the Layer 1 [`DrainSignal`].
    fn on_upgrade(
        &self,
        io: BoxStreamIo,
        leftover: Vec<u8>,
        drain: Arc<dyn DrainSignal>,
    ) -> BoxFuture<'static, ()>;
}

/// HTTP message payload representation supporting zero-buffer streaming and protocol upgrade.
pub enum BodyKind {
    Empty,
    Bytes(Vec<u8>),
    Stream {
        reader: Box<dyn AsyncByteStream>,
        content_length: Option<u64>,
        /// When `true`, filters must not whole-buffer or whole-compress the body,
        /// and the HTTP writer flushes each chunk immediately.
        streaming: bool,
    },
    /// Protocol upgrade handoff triggered after writing the `101` response headers.
    Upgrade(Arc<dyn UpgradeHandler>),
}

impl fmt::Debug for BodyKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("BodyKind::Empty"),
            Self::Bytes(b) => write!(f, "BodyKind::Bytes({} bytes)", b.len()),
            Self::Stream {
                content_length,
                streaming,
                ..
            } => f
                .debug_struct("BodyKind::Stream")
                .field("content_length", content_length)
                .field("streaming", streaming)
                .finish_non_exhaustive(),
            Self::Upgrade(_) => f.write_str("BodyKind::Upgrade(..)"),
        }
    }
}

impl BodyKind {
    /// Return `true` when the body is empty.
    pub fn is_empty(&self) -> bool {
        matches!(self, Self::Empty)
    }

    /// Return `true` if this is a live unbuffered stream or upgraded tunnel.
    pub fn is_streaming(&self) -> bool {
        matches!(
            self,
            Self::Stream {
                streaming: true,
                ..
            } | Self::Upgrade(_)
        )
    }

    /// Return known fixed content length when available.
    pub fn content_length(&self) -> Option<u64> {
        match self {
            Self::Empty => Some(0),
            Self::Bytes(b) => Some(b.len() as u64),
            Self::Stream { content_length, .. } => *content_length,
            Self::Upgrade(_) => None,
        }
    }

    /// Collect the entire body into memory (primarily for small responses and tests).
    pub fn into_bytes(self) -> BoxFuture<'static, io::Result<Vec<u8>>> {
        Box::pin(async move {
            match self {
                Self::Empty | Self::Upgrade(_) => Ok(Vec::new()),
                Self::Bytes(b) => Ok(b),
                Self::Stream { mut reader, .. } => {
                    let mut out = Vec::new();
                    loop {
                        let next =
                            poll_fn(|cx| Pin::new(&mut *reader).poll_next(cx))
                                .await;
                        match next {
                            Some(Ok(chunk)) => out.extend_from_slice(&chunk),
                            Some(Err(err)) => return Err(err),
                            None => return Ok(out),
                        }
                    }
                }
            }
        })
    }
}

/// Split an HTTP request target (`path?query`) into `(path, Option<query>)`.
pub fn split_uri_target(raw_uri: &str) -> (String, Option<String>) {
    let target = raw_uri.split('#').next().unwrap_or(raw_uri);
    match target.split_once('?') {
        Some((p, q)) => (p.to_owned(), Some(q.to_owned())),
        None => (target.to_owned(), None),
    }
}

/// Incoming HTTP request delivered through the router and filter pipeline.
#[derive(Debug)]
pub struct HttpRequest {
    pub method: Method,
    pub raw_uri: String,
    pub path: String,
    pub query: Option<String>,
    pub path_params: BTreeMap<String, String>,
    pub version: Version,
    pub headers: Headers,
    pub body: BodyKind,
    pub remote_addr: Option<SocketAddr>,
}

impl HttpRequest {
    /// Construct a request targeting `raw_uri` with `HTTP/1.1` and empty body.
    pub fn new(method: Method, raw_uri: impl Into<String>) -> Self {
        let raw_uri = raw_uri.into();
        let (path, query) = split_uri_target(&raw_uri);
        Self {
            method,
            raw_uri,
            path,
            query,
            path_params: BTreeMap::new(),
            version: Version::Http11,
            headers: Headers::new(),
            body: BodyKind::Empty,
            remote_addr: None,
        }
    }

    /// Look up a route path parameter extracted by [`RouteMatch::Param`].
    pub fn path_param(&self, key: &str) -> Option<&str> {
        self.path_params.get(key).map(String::as_str)
    }

    /// Insert or replace a header and return `self`.
    pub fn with_header(
        mut self,
        name: impl Into<String>,
        value: impl Into<String>,
    ) -> Self {
        self.headers.insert(name, value);
        self
    }

    /// Set the request body and return `self`.
    pub fn with_body(mut self, body: BodyKind) -> Self {
        self.body = body;
        self
    }

    /// Extract a decoded query parameter matching `key`.
    pub fn query_param(&self, key: &str) -> Option<String> {
        extract_query_param(self.query.as_deref(), key)
    }

    /// Snapshot immutable request metadata for response-phase filters.
    pub fn request_meta(&self) -> RequestMeta {
        RequestMeta {
            method: self.method.clone(),
            path: self.path.clone(),
            query: self.query.clone(),
            version: self.version,
            headers: self.headers.clone(),
            is_head: self.method == Method::Head,
            remote_addr: self.remote_addr,
            started_at: std::time::Instant::now(),
        }
    }
}

fn extract_query_param(query: Option<&str>, key: &str) -> Option<String> {
    let query = query?;
    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (k_raw, v_raw) = pair.split_once('=').unwrap_or((pair, ""));
        let k = percent_decode_query(k_raw);
        if k == key {
            return Some(percent_decode_query(v_raw));
        }
    }
    None
}

impl RequestMeta {
    /// Extract a decoded query parameter matching `key`.
    pub fn query_param(&self, key: &str) -> Option<String> {
        extract_query_param(self.query.as_deref(), key)
    }
}

fn percent_decode_query(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                if let (Some(hi), Some(lo)) =
                    (hex_val(bytes[i + 1]), hex_val(bytes[i + 2]))
                {
                    out.push((hi << 4) | lo);
                    i += 3;
                } else {
                    out.push(b'%');
                    i += 1;
                }
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

const fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Request summary preserved across handler execution for response filters.
#[derive(Debug, Clone)]
pub struct RequestMeta {
    pub method: Method,
    pub path: String,
    pub query: Option<String>,
    pub version: Version,
    pub headers: Headers,
    pub is_head: bool,
    pub remote_addr: Option<SocketAddr>,
    pub started_at: std::time::Instant,
}

/// Outgoing HTTP response returned by handlers and filters.
#[derive(Debug)]
pub struct HttpResponse {
    pub status: StatusCode,
    pub headers: Headers,
    pub body: BodyKind,
}

impl HttpResponse {
    /// Construct a response with an empty body.
    pub fn empty(status: StatusCode) -> Self {
        Self {
            status,
            headers: Headers::new(),
            body: BodyKind::Empty,
        }
    }

    /// Construct a byte response with explicit `Content-Type`.
    pub fn bytes(
        status: StatusCode,
        content_type: impl Into<String>,
        data: impl Into<Vec<u8>>,
    ) -> Self {
        let data = data.into();
        let mut headers = Headers::with_capacity(2);
        headers.insert("Content-Type", content_type);
        headers.insert("Content-Length", data.len().to_string());
        Self {
            status,
            headers,
            body: BodyKind::Bytes(data),
        }
    }

    /// Construct an HTML response with `text/html; charset=utf-8`.
    pub fn html(status: StatusCode, html: impl Into<String>) -> Self {
        Self::bytes(
            status,
            "text/html; charset=utf-8",
            html.into().into_bytes(),
        )
    }

    /// Construct a plain-text response with `text/plain; charset=utf-8`.
    pub fn text(status: StatusCode, text: impl Into<String>) -> Self {
        Self::bytes(
            status,
            "text/plain; charset=utf-8",
            text.into().into_bytes(),
        )
    }

    /// Construct a `301 Moved Permanently` redirect pointing at `location`.
    pub fn redirect_moved(location: impl Into<String>) -> Self {
        let mut resp = Self::empty(StatusCode::MOVED_PERMANENTLY);
        resp.headers.insert("Location", location);
        resp
    }

    /// Construct a streaming response.
    pub fn stream(
        status: StatusCode,
        content_type: impl Into<String>,
        reader: Box<dyn AsyncByteStream>,
        content_length: Option<u64>,
        streaming: bool,
    ) -> Self {
        let mut headers = Headers::with_capacity(2);
        headers.insert("Content-Type", content_type);
        if let Some(len) = content_length {
            headers.insert("Content-Length", len.to_string());
        }
        Self {
            status,
            headers,
            body: BodyKind::Stream {
                reader,
                content_length,
                streaming,
            },
        }
    }

    /// Construct a protocol upgrade response (such as `101 Switching Protocols`).
    pub fn upgrade(
        status: StatusCode,
        headers: Headers,
        handler: Arc<dyn UpgradeHandler>,
    ) -> Self {
        Self {
            status,
            headers,
            body: BodyKind::Upgrade(handler),
        }
    }

    /// Insert or replace a response header and return `self`.
    pub fn with_header(
        mut self,
        name: impl Into<String>,
        value: impl Into<String>,
    ) -> Self {
        self.headers.insert(name, value);
        self
    }

    /// Return the first header value matching `name`.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name)
    }
}

/// Path matching rule for an HTTP route.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteMatch {
    /// Matches an exact decoded/normalized request path.
    Exact(String),
    /// Matches path segments with `:param` single-segment captures and trailing `*rest` capture.
    Param(String),
    /// Matches requests whose path starts with this prefix.
    Prefix(String),
    /// Matches any remaining path as fallback.
    CatchAll,
}

/// Declarative specification for mounting a route onto [`RouterService`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteSpec {
    pub id: String,
    pub host: Option<String>,
    pub matcher: RouteMatch,
    pub methods: Option<Vec<Method>>,
    pub priority: i32,
}

impl RouteSpec {
    /// Construct a route catching all methods and hosts matching `matcher`.
    pub fn new(id: impl Into<String>, matcher: RouteMatch) -> Self {
        Self {
            id: id.into(),
            host: None,
            matcher,
            methods: None,
            priority: 0,
        }
    }
}

/// Endpoint handler invoked when a mounted route matches.
pub trait HttpHandler: Send + Sync + 'static {
    /// Process the request and yield an HTTP response.
    fn call(&self, req: HttpRequest) -> BoxFuture<'static, HttpResponse>;
}

/// Result of running a request-phase filter.
#[derive(Debug)]
pub enum FilterOutcome {
    /// Pass the (possibly modified) request down the filter chain.
    Continue(HttpRequest),
    /// Short-circuit the pipeline and return an immediate response.
    Respond(HttpResponse),
}

/// Bidirectional HTTP filter mounted into [`RouterService`].
pub trait HttpFilter: Send + Sync + 'static {
    /// Diagnostic name used in effects and inspection snapshots.
    fn name(&self) -> &str;

    /// Ordering priority: smaller values run earlier on request and later on response.
    fn priority(&self) -> i32 {
        0
    }

    /// Intercept or transform an incoming request before routing.
    fn on_request(&self, req: HttpRequest)
    -> BoxFuture<'static, FilterOutcome>;

    /// Transform an outgoing response after the handler (or short-circuit) runs.
    fn on_response(
        &self,
        req_meta: &RequestMeta,
        resp: HttpResponse,
    ) -> BoxFuture<'static, HttpResponse>;
}

/// Bound HTTP listener metadata published when a listener starts serving.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundServerInfo {
    pub local_addr: SocketAddr,
}

impl Service for BoundServerInfo {
    const NAME: &'static str = "http.bound_info";
}

/// Layer 2 router contract trait separating control-plane mutations from
/// snapshot dispatch on the data plane.
pub trait Router: Send + Sync + 'static {
    /// Mount a route owned by `caller`, returning an [`EffectHandle`] whose disposal removes it.
    fn mount_route(
        &self,
        caller: &FrameContext,
        spec: RouteSpec,
        handler: Arc<dyn HttpHandler>,
    ) -> Result<EffectHandle, FrameError>;

    /// Mount a bidirectional filter owned by `caller`, returning an [`EffectHandle`] whose disposal removes it.
    fn mount_filter(
        &self,
        caller: &FrameContext,
        filter: Arc<dyn HttpFilter>,
    ) -> Result<EffectHandle, FrameError>;

    /// Dispatch a single HTTP request against the current routing snapshot.
    fn dispatch(&self, req: HttpRequest) -> BoxFuture<'static, HttpResponse>;
}

/// Frame service handle wrapping any [`Router`] implementation.
#[derive(Clone)]
pub struct RouterService(pub Arc<dyn Router>);

impl RouterService {
    /// Wrap a [`Router`] implementation as a registered service handle.
    pub fn new(inner: Arc<dyn Router>) -> Self {
        Self(inner)
    }

    /// Delegate [`Router::mount_route`] with explicit caller ownership.
    pub fn mount_route(
        &self,
        caller: &FrameContext,
        spec: RouteSpec,
        handler: Arc<dyn HttpHandler>,
    ) -> Result<EffectHandle, FrameError> {
        self.0.mount_route(caller, spec, handler)
    }

    /// Delegate [`Router::mount_filter`] with explicit caller ownership.
    pub fn mount_filter(
        &self,
        caller: &FrameContext,
        filter: Arc<dyn HttpFilter>,
    ) -> Result<EffectHandle, FrameError> {
        self.0.mount_filter(caller, filter)
    }

    /// Delegate [`Router::dispatch`].
    pub fn dispatch(
        &self,
        req: HttpRequest,
    ) -> BoxFuture<'static, HttpResponse> {
        self.0.dispatch(req)
    }
}

/// Caller-bound [`RouterService`] view obtained via `ctx.use_service::<RouterService>()`.
#[derive(Clone)]
pub struct ScopedRouter {
    inner: Arc<RouterService>,
    caller: FrameContext,
}

impl ScopedRouter {
    /// Mount a route whose lifetime is bound directly to the caller's fiber.
    pub fn mount_route(
        &self,
        spec: RouteSpec,
        handler: Arc<dyn HttpHandler>,
    ) -> Result<EffectHandle, FrameError> {
        self.inner.mount_route(&self.caller, spec, handler)
    }

    /// Mount a filter whose lifetime is bound directly to the caller's fiber.
    pub fn mount_filter(
        &self,
        filter: Arc<dyn HttpFilter>,
    ) -> Result<EffectHandle, FrameError> {
        self.inner.mount_filter(&self.caller, filter)
    }

    /// Dispatch a request against the current immutable snapshot.
    pub fn dispatch(
        &self,
        req: HttpRequest,
    ) -> BoxFuture<'static, HttpResponse> {
        self.inner.dispatch(req)
    }
}

impl Service for RouterService {
    const NAME: &'static str = "http.router";
}

impl ScopedService for RouterService {
    type View = ScopedRouter;

    fn bind(self: Arc<Self>, caller: FrameContext) -> Self::View {
        ScopedRouter {
            inner: self,
            caller,
        }
    }
}
