//! Minimal white (`#ffffff`), zero-CDN HTTP + SSE timeline + WebSocket inspector UI & API.

use frame::BoxFuture;
use http_api::{
    AsyncByteStream, BodyKind, FilterOutcome, HttpFilter, HttpHandler,
    HttpRequest, HttpResponse, Method, RequestMeta, RouterService, StatusCode,
};
use std::collections::VecDeque;
use std::fmt::Write as _;
use std::io;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context as TaskContext, Poll};
use std::time::Instant;

/// Single recorded SSE chunk arrival inside [`InspectRecord`].
#[derive(Debug, Clone)]
pub struct SseChunkEntry {
    pub offset_ms: u64,
    pub bytes: usize,
    pub preview: String,
}

/// Recorded HTTP exchange with optional streaming SSE chunk timeline.
#[derive(Debug, Clone)]
pub struct InspectRecord {
    pub id: u64,
    pub method: Method,
    pub path: String,
    pub query: Option<String>,
    pub status: u16,
    pub elapsed_ms: u64,
    pub req_headers: Vec<(String, String)>,
    pub resp_headers: Vec<(String, String)>,
    pub sse_chunks: Vec<SseChunkEntry>,
}

pub(crate) struct InspectStore {
    next_id: AtomicU64,
    max_entries: usize,
    records: Mutex<VecDeque<InspectRecord>>,
}

impl InspectStore {
    pub(crate) fn new(max_entries: usize) -> Arc<Self> {
        Arc::new(Self {
            next_id: AtomicU64::new(1),
            max_entries: max_entries.max(8),
            records: Mutex::new(VecDeque::new()),
        })
    }

    pub(crate) fn push_record(&self, mut rec: InspectRecord) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        rec.id = id;
        let mut guard = self.records.lock().unwrap_or_else(|e| e.into_inner());
        while guard.len() >= self.max_entries {
            guard.pop_front();
        }
        guard.push_back(rec);
        id
    }

    pub(crate) fn append_sse_chunk(&self, id: u64, chunk: SseChunkEntry) {
        let mut guard = self.records.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(rec) = guard.iter_mut().find(|r| r.id == id)
            && rec.sse_chunks.len() < 128
        {
            rec.sse_chunks.push(chunk);
        }
    }

    pub(crate) fn get(&self, id: u64) -> Option<InspectRecord> {
        let guard = self.records.lock().unwrap_or_else(|e| e.into_inner());
        guard.iter().find(|r| r.id == id).cloned()
    }

    pub(crate) fn snapshot(&self) -> Vec<InspectRecord> {
        self.records
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .cloned()
            .collect()
    }
}

struct InspectStreamTap {
    inner: Box<dyn AsyncByteStream>,
    id: u64,
    started: Instant,
    store: Arc<InspectStore>,
}

impl AsyncByteStream for InspectStreamTap {
    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
    ) -> Poll<Option<io::Result<Vec<u8>>>> {
        let poll = Pin::new(&mut *self.inner).poll_next(cx);
        if let Poll::Ready(Some(Ok(ref bytes))) = poll {
            let preview_len = bytes.len().min(96);
            let preview =
                String::from_utf8_lossy(&bytes[..preview_len]).into_owned();
            self.store.append_sse_chunk(
                self.id,
                SseChunkEntry {
                    offset_ms: self.started.elapsed().as_millis() as u64,
                    bytes: bytes.len(),
                    preview,
                },
            );
        }
        poll
    }
}

pub(crate) struct InspectFilter {
    pub(crate) inspect_prefix: String,
    pub(crate) store: Arc<InspectStore>,
}

impl HttpFilter for InspectFilter {
    fn name(&self) -> &str {
        "ext-dev-kit-inspect"
    }

    fn priority(&self) -> i32 {
        -900
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
        if req_meta.path.starts_with(&self.inspect_prefix) {
            return Box::pin(async move { resp });
        }
        let rec = InspectRecord {
            id: 0,
            method: req_meta.method.clone(),
            path: req_meta.path.clone(),
            query: req_meta.query.clone(),
            status: resp.status.as_u16(),
            elapsed_ms: req_meta.started_at.elapsed().as_millis() as u64,
            req_headers: req_meta
                .headers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            resp_headers: resp
                .headers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            sse_chunks: Vec::new(),
        };
        let id = self.store.push_record(rec);

        match std::mem::replace(&mut resp.body, BodyKind::Empty) {
            BodyKind::Stream {
                reader,
                content_length,
                streaming: true,
            } => {
                resp.body = BodyKind::Stream {
                    reader: Box::new(InspectStreamTap {
                        inner: reader,
                        id,
                        started: req_meta.started_at,
                        store: Arc::clone(&self.store),
                    }),
                    content_length,
                    streaming: true,
                };
            }
            other => {
                resp.body = other;
            }
        }

        Box::pin(async move { resp })
    }
}

fn json_esc(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len() + 2);
    out.push('"');
    for ch in raw.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn html_esc(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        match ch {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            c => out.push(c),
        }
    }
    out
}

pub(crate) struct InspectUiHandler {
    pub(crate) store: Arc<InspectStore>,
    pub(crate) base_path: String,
}

impl HttpHandler for InspectUiHandler {
    fn call(&self, _req: HttpRequest) -> BoxFuture<'static, HttpResponse> {
        let rows = self.store.snapshot();
        let base_path = self.base_path.clone();
        Box::pin(async move {
            let mut html = String::with_capacity(2048);
            html.push_str(
                "<!doctype html><html><head><meta charset=\"utf-8\">\
                 <title>d Request & Stream Inspector</title>\
                 <style>body{background:#ffffff;color:#1f2328;font-family:system-ui,sans-serif;margin:24px auto;max-width:980px;padding:0 16px}\
                 h1{font-size:18px;border-bottom:1px solid #e5e7eb;padding-bottom:8px}\
                 table{width:100%;border-collapse:collapse;font-size:13px}\
                 th,td{text-align:left;padding:8px;border-bottom:1px solid #e5e7eb;vertical-align:top}\
                 th{background:#f6f8fa;color:#656d76}\
                 code{background:#f6f8fa;padding:2px 4px;border-radius:3px}\
                 button{background:#ffffff;border:1px solid #d0d7de;padding:3px 8px;color:#0969da;cursor:pointer}\
                 </style></head><body><h1>Request & Stream Inspector</h1>",
            );
            let _ = write!(
                html,
                "<p style=\"color:#656d76;font-size:13px\">Events: {} | JSON API: <a href=\"{base_path}/api/events\">{base_path}/api/events</a></p>",
                rows.len()
            );
            html.push_str("<table><thead><tr><th>ID</th><th>Request</th><th>Status</th><th>ms</th><th>SSE Timeline</th><th>Action</th></tr></thead><tbody>");
            for r in rows.iter().rev() {
                let target = match &r.query {
                    Some(q) if !q.is_empty() => format!("{}?{}", r.path, q),
                    _ => r.path.clone(),
                };
                let mut sse_summary = String::new();
                for c in &r.sse_chunks {
                    let _ = write!(
                        sse_summary,
                        "<div><code>+{}ms ({}B)</code> {}</div>",
                        c.offset_ms,
                        c.bytes,
                        html_esc(&c.preview)
                    );
                }
                let _ = write!(
                    html,
                    "<tr><td>#{}</td><td><code>{} {}</code></td><td>{}</td><td>{}</td><td>{}</td>\
                     <td><form method=\"post\" action=\"{base_path}/api/replay?id={}\"><button type=\"submit\">Replay</button></form></td></tr>",
                    r.id,
                    html_esc(r.method.as_str()),
                    html_esc(&target),
                    r.status,
                    r.elapsed_ms,
                    if sse_summary.is_empty() {
                        "-"
                    } else {
                        &sse_summary
                    },
                    r.id
                );
            }
            html.push_str("</tbody></table></body></html>");
            HttpResponse::html(StatusCode::OK, html)
        })
    }
}

pub(crate) struct InspectEventsJsonHandler {
    pub(crate) store: Arc<InspectStore>,
}

impl HttpHandler for InspectEventsJsonHandler {
    fn call(&self, _req: HttpRequest) -> BoxFuture<'static, HttpResponse> {
        let rows = self.store.snapshot();
        Box::pin(async move {
            let mut out = String::from("{\"events\":[");
            for (idx, r) in rows.iter().enumerate() {
                if idx > 0 {
                    out.push(',');
                }
                let _ = write!(
                    out,
                    "{{\"id\":{},\"method\":{},\"path\":{},\"status\":{},\"elapsed_ms\":{},\"sse_chunks\":[",
                    r.id,
                    json_esc(r.method.as_str()),
                    json_esc(&r.path),
                    r.status,
                    r.elapsed_ms
                );
                for (cidx, c) in r.sse_chunks.iter().enumerate() {
                    if cidx > 0 {
                        out.push(',');
                    }
                    let _ = write!(
                        out,
                        "{{\"offset_ms\":{},\"bytes\":{},\"preview\":{}}}",
                        c.offset_ms,
                        c.bytes,
                        json_esc(&c.preview)
                    );
                }
                out.push_str("]}");
            }
            out.push_str("]}");
            HttpResponse::bytes(
                StatusCode::OK,
                "application/json",
                out.into_bytes(),
            )
        })
    }
}

pub(crate) struct InspectReplayHandler {
    pub(crate) store: Arc<InspectStore>,
    pub(crate) router: RouterService,
}

impl HttpHandler for InspectReplayHandler {
    fn call(&self, req: HttpRequest) -> BoxFuture<'static, HttpResponse> {
        let store = Arc::clone(&self.store);
        let router = self.router.clone();
        Box::pin(async move {
            let Some(id) =
                req.query_param("id").and_then(|s| s.parse::<u64>().ok())
            else {
                return HttpResponse::text(
                    StatusCode::BAD_REQUEST,
                    "missing valid ?id=",
                );
            };
            let Some(rec) = store.get(id) else {
                return HttpResponse::text(
                    StatusCode::NOT_FOUND,
                    "recorded request not found",
                );
            };
            let uri = match &rec.query {
                Some(q) if !q.is_empty() => format!("{}?{}", rec.path, q),
                _ => rec.path.clone(),
            };
            let mut replay_req = HttpRequest::new(rec.method, uri);
            for (k, v) in rec.req_headers {
                replay_req.headers.append(k, v);
            }
            router.dispatch(replay_req).await
        })
    }
}
